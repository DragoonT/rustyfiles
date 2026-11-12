//! The window both of the dock's icon pop-ups (the app menu and the hover preview) are shown in: a transparent GPUI pop-up made hidden,
//! kept after its first use and shown, moved and hidden by `rf_win::dock::panel::{show_at, hide}`. It is made WHEN FIRST NEEDED (a
//! right click, a pointer that has rested on an icon), never at the dock's start: a hidden window costs memory and a swap chain for
//! nothing until someone asks for it.

use gpui_kit::*;
use rf_win::dock::layout::Rect;

/// Makes a hidden pop-up window of `initial` logical px near `near` (device px; the real place is set by `show_at`). `on_close` is
/// what Alt+F4 does in it: the window is never destroyed by the user (a destroyed window is never opened again), only put away.
pub fn open_hidden<V: Render + 'static>(
    initial: (f32, f32),
    display: Option<DisplayId>,
    near: Rect,
    scale: f32,
    on_close: fn(&mut App),
    build: impl FnOnce(&mut Window, &mut Context<V>) -> V + 'static,
    cx: &mut App,
) -> Option<WindowHandle<V>> {
    let (x, y, _, _) = super::plan::logical(near, scale);
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(point(px(x), px(y)), size(px(initial.0), px(initial.1))))),
        display_id: display,
        titlebar: None,
        focus: false,
        show: false,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        window_background: WindowBackgroundAppearance::Transparent,
        ..Default::default()
    };
    match cx.open_window(opts, move |window, cx| cx.new(|cx| build(window, cx))) {
        Ok(h) => {
            let _ = h.update(cx, |_, window, cx| {
                window.on_window_should_close(cx, move |_, cx| {
                    cx.defer(on_close);
                    false
                });
            });
            Some(h)
        }
        Err(e) => {
            log::error!("dock: cannot make a pop-up window: {e:#}");
            None
        }
    }
}

/// The primary monitor's scale, its work area and the dock's middle glass (device px): what a pop-up above an icon is placed from.
/// `None` before the dock has a layout.
pub fn anchor_basis(cx: &App) -> Option<(f32, Rect, Rect)> {
    let (scale, work) = super::host::monitor_work(cx)?;
    let glass = super::host::visible_glass_rect(super::host::Kind::Middle, cx)?;
    Some((scale, work, glass))
}
