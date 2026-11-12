//! The few pieces the clock's panel (`cal_view.rs`) and the bell's panel (`notif_view.rs`) share, in the look of the control centre's:
//! the colours of one frame, a round or flat button with the keyboard ring, and opening a page of Windows' settings.

use std::path::Path;
use std::time::Duration;

use gpui_kit::assets::IconName as L;
use gpui_kit::component::Icon;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::look::Palette;
use crate::theme::Palette as ThemePalette;

/// The colours of one frame.
pub struct Look {
    pub pal: Palette,
    /// The system dark look (light text on dark glass).
    pub dark: bool,
    pub accent: Hsla,
    /// The glyph or text on a surface filled with the accent: dark on a light accent, white on a deep one.
    pub on_accent: Hsla,
}

impl Look {
    /// The island's palette at this moment decides the tone; the app's own palette supplies the accent (lifted on dark, as everywhere
    /// in the app) and the colour that goes on it.
    pub fn of(pal: Palette) -> Look {
        let dark = pal.mix < 0.5;
        let theme = if dark { ThemePalette::dark() } else { ThemePalette::light() };
        Look { pal, dark, accent: theme.accent, on_accent: theme.accent_text }
    }
}

/// A round button with a glyph. `focus` makes it a keyboard stop (a ring in the accent colour while it has the focus by key), `tip`
/// names it for the pointer, `enabled` false greys it and makes it inert.
pub fn round_button(id: impl Into<ElementId>, icon: L, size: f32, look: &Look, focus: Option<&FocusHandle>, tip: Option<&'static str>, enabled: bool) -> Stateful<Div> {
    let (hover, pressed) = (look.pal.hover_fill().hsla(), look.pal.pressed_fill().hsla());
    let ring = look.accent;
    let glyph = if enabled { look.pal.text().hsla() } else { look.pal.text_secondary().hsla().opacity(0.4) };
    div()
        .id(id)
        .size(px(size))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .border_2()
        .border_color(transparent_black())
        .when(enabled, |d| {
            d.cursor_pointer()
                .hover(move |s| s.bg(hover))
                .active(move |s| s.bg(pressed))
                .when_some(focus, |d, f| d.track_focus(f).focus_visible(move |s| s.border_color(ring)))
                .when_some(tip, |d, tip| d.tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(tip).build(w, cx)))
        })
        .child(Icon::new(icon).size(px(size * 0.5)).text_color(glyph))
}

/// Opens a Windows page (`ms-settings:...`) through the shell, on the one shell thread and from a thread that may wait for it.
pub fn open_uri(uri: &'static str) {
    std::thread::spawn(move || {
        let ok = rf_win::sta::call(Duration::from_secs(15), move || rf_win::shell::open(Path::new(uri))).and_then(Result::ok);
        if ok.is_none() {
            log::warn!("dock panel: cannot open {uri}");
        }
    });
}
