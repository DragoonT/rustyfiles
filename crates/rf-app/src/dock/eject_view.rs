//! Safe removal, in a panel of our own: the right island's removal button opens it, each removable drive has a row with its
//! letter, its label and a "Çıkar" button, and a drive that is in use says so in its row.
//!
//! The user asked for it as OUR window ("aygıt çıkarma bizim penceremiz ile olsun"), not Windows' own notification icon and its
//! menu, which reach a taskbar that is not Explorer's only by luck. The drives are read by the right island (every few seconds,
//! off the UI thread) and kept in [`Removables`]; this view draws them and ejects one the way the file manager does
//! (`rf_win::volume::eject`: the shell's own verb, on the shell thread).
//!
//! NOT COVERED YET: a USB hard disk reports itself as a fixed drive and the other hot-plug devices Windows' own list has are not
//! drives at all; only what `rf_win::volume::removable_drives` gives is here.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName as L;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rf_win::dock::panel::{self as geo, CloseCause};

use super::backdrop::IslandGlass;
use super::panel_parts::Look;
use crate::glass_watch::{self, PopupGlass};
use crate::t;

/// The panel's width, and the heights of a row and of the padding around the rows (logical px, the monitor's scale).
pub const WIDTH: f32 = 300.0;
pub const ROW_H: f32 = 48.0;
pub const PAD: f32 = 8.0;
/// The line that says nothing can be removed.
pub const EMPTY_H: f32 = 44.0;
/// How long a failed removal is shown on its row.
pub const FAILED_FOR: Duration = Duration::from_secs(6);

/// The removable drives that hold a volume now (letter, label), as the right island last read them, and what the last removal
/// came to. The island and the panel both look here.
#[derive(Default)]
pub struct Removables {
    pub drives: Vec<(char, String)>,
    /// The drive being removed now.
    pub busy: Option<char>,
    /// A drive that could not be removed a moment ago (it is in use).
    pub failed: Option<(char, Instant)>,
}

impl Global for Removables {}

pub fn drives(cx: &App) -> Vec<(char, String)> {
    cx.try_global::<Removables>().map_or_else(Vec::new, |r| r.drives.clone())
}

/// The drives were read again. True when they changed.
pub fn set_drives(drives: Vec<(char, String)>, cx: &mut App) -> bool {
    if !cx.has_global::<Removables>() {
        cx.set_global(Removables::default());
    }
    let failed_over = cx.global::<Removables>().failed.is_some_and(|(_, at)| at.elapsed() >= FAILED_FOR);
    if cx.global::<Removables>().drives == drives && !failed_over {
        return false;
    }
    cx.update_global::<Removables, _>(|r, _| {
        r.drives = drives;
        if failed_over {
            r.failed = None;
        }
    });
    true
}

/// Reads the removable drives (blocks briefly: off the UI thread). `RUSTYFILES_EJECT_FAKE=1` gives one made-up drive instead, so
/// that the button and the panel can be looked at on a PC with nothing plugged in; removing it fails like a drive in use.
pub fn read_drives() -> Vec<(char, String)> {
    if std::env::var_os("RUSTYFILES_EJECT_FAKE").is_some() {
        return vec![('Z', "DENEME".to_string())];
    }
    rf_win::volume::removable_drives()
}

/// The panel's size for this many drives.
pub fn size_for(count: usize) -> (f32, f32) {
    (WIDTH, 2.0 * PAD + if count == 0 { EMPTY_H } else { count as f32 * ROW_H })
}

/// What a drive's row says: its label and letter ("KINGSTON (E:)"), or the letter alone for a drive without a label.
pub fn row_title(letter: char, label: &str) -> String {
    let label = label.trim();
    if label.is_empty() { format!("{letter}:") } else { format!("{label} ({letter}:)") }
}

pub struct EjectView {
    island: Rc<RefCell<IslandGlass>>,
    hwnd: isize,
    popup: Option<PopupGlass>,
    _watch: Option<Task<()>>,
    shown: bool,
    closing: bool,
    anim: Option<(Instant, bool)>,
    focus: FocusHandle,
    reported: (f32, f32),
    _obs: Subscription,
}

impl EjectView {
    pub fn new(island: Rc<RefCell<IslandGlass>>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        if !cx.has_global::<Removables>() {
            cx.set_global(Removables::default());
        }
        // a drive came or went, or a removal ended: the rows and the window's size follow (only while the panel is open)
        let obs = cx.observe_global::<Removables>(|this: &mut Self, cx| {
            if this.shown {
                this.refit(cx);
                cx.notify();
            }
        });
        EjectView {
            island,
            hwnd: crate::look::window::hwnd_of(window),
            popup: None,
            _watch: None,
            shown: false,
            closing: false,
            anim: None,
            focus: cx.focus_handle(),
            reported: size_for(drives(cx).len()),
            _obs: obs,
        }
    }

    pub fn size(&self, cx: &App) -> (f32, f32) {
        size_for(drives(cx).len())
    }

    fn live(&self) -> bool {
        self.shown && !self.closing
    }

    fn refit(&mut self, cx: &mut Context<Self>) {
        let size = self.size(cx);
        if size != self.reported {
            self.reported = size;
            super::panel::relayout_sized(super::panel::EJECT, size, cx);
        }
    }

    // ------------------------------------------------------------------ opening and closing (called by panel.rs)

    pub fn on_open(&mut self, _dark: bool, _window: &mut Window, cx: &mut Context<Self>) {
        self.shown = true;
        self.closing = false;
        self.anim = Some((Instant::now(), true));
        self.reported = self.size(cx);
        cx.notify();
    }

    pub fn on_reopen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.shown = true;
        self.closing = false;
        self.anim = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub fn on_shown(&mut self, dark: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.popup.is_none() && self.shown {
            // an island growing into this panel: its glass is a shape in the window that the growth moves (grow_host.rs)
            self.popup = Some(match super::grow_host::shape_for_attach(super::panel::EJECT, cx) {
                Some((shape, radius)) => PopupGlass::start_panel_shaped(self.hwnd, dark, shape, radius),
                None => PopupGlass::start_panel(self.hwnd, dark),
            });
            self._watch = Some(glass_watch::watch(cx, |this: &mut Self| this.popup.as_mut()));
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub fn on_close(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.closing = true;
        self.anim = Some((Instant::now(), false));
        cx.notify();
    }

    pub fn on_hidden(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.shown = false;
        self.closing = false;
        self.anim = None;
        self.popup = None;
        self._watch = None;
        cx.notify();
    }

    /// "Çıkar": the drive is removed the way the file manager removes it; the drives are read again when the shell has answered.
    fn eject(&mut self, letter: char, cx: &mut Context<Self>) {
        if !self.live() || cx.global::<Removables>().busy.is_some() {
            return;
        }
        cx.update_global::<Removables, _>(|r, _| {
            r.busy = Some(letter);
            r.failed = None;
        });
        cx.spawn(async move |_, cx| {
            let done = cx
                .background_executor()
                .spawn(async move { rf_win::sta::call(Duration::from_secs(30), move || rf_win::volume::eject(letter).map_err(|e| e.to_string())) })
                .await;
            let failed = !matches!(done, Some(Ok(())));
            if failed {
                log::warn!("dock: a drive could not be removed safely: {done:?}");
            }
            let now = cx.background_executor().spawn(async { read_drives() }).await;
            cx.update(|cx| {
                // (a drive that is still there after a removal that "worked" is in use after all)
                let still = now.iter().any(|(l, _)| *l == letter);
                let left = now.len();
                cx.update_global::<Removables, _>(|r, _| {
                    r.busy = None;
                    r.failed = (failed || still).then(|| (letter, Instant::now()));
                    r.drives = now;
                });
                // the island's button follows (it goes when the last drive went), and the panel closes when nothing is left
                for island in super::host::right_islands(cx) {
                    island.update(cx, |me, cx| me.drives_changed(cx));
                }
                if left == 0 {
                    super::panel::close(CloseCause::Chosen, cx);
                }
            });
        })
        .detach();
        cx.notify();
    }
}

impl Render for EjectView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.shown {
            return div().size_full().into_any_element();
        }
        let (pal, _) = super::host::frame_palette(&self.island, window);
        let look = Look::of(pal);
        let grown = super::grow_host::view_frame(super::panel::EJECT, cx);
        let fill = glass_watch::panel_fill(look.dark, grown.map_or_else(|| self.popup.as_ref().is_none_or(|g| g.fallback()), |g| g.plain));
        let frame = match (self.anim, grown) {
            (_, Some(_)) | (None, None) => geo::Frame { opacity: 1.0, drop_px: 0.0, done: true },
            (Some((t, opening)), None) => geo::frame(t.elapsed().as_millis() as u32, opening, crate::settings::animations()),
        };
        if !frame.done || grown.is_some_and(|g| g.moving) {
            window.request_animation_frame();
        }
        let state = cx.global::<Removables>();
        let (busy, failed) = (state.busy, state.failed.filter(|(_, at)| at.elapsed() < FAILED_FOR).map(|(l, _)| l));
        let list = state.drives.clone();
        let (text, secondary) = (look.pal.text().hsla(), look.pal.text_secondary().hsla());
        let (hover, pressed, field) = (look.pal.hover_fill().hsla(), look.pal.press_plate().hsla(), look.pal.field_fill().hsla());
        let danger: Hsla = gpui_kit::rgb(0xE5484D).into();

        let mut body = v_flex().size_full().p(px(PAD)).opacity(frame.opacity).mt(px(frame.drop_px));
        if list.is_empty() {
            body = body.child(div().h(px(EMPTY_H)).flex().items_center().justify_center().text_color(secondary).child(t!("Çıkarılabilecek aygıt yok")));
        }
        for (letter, label) in list {
            let is_busy = busy == Some(letter);
            let is_failed = failed == Some(letter);
            let under = if is_busy {
                t!("Çıkarılıyor…")
            } else if is_failed {
                t!("Kullanımda: açık dosyaları kapatıp yeniden deneyin")
            } else {
                t!("Çıkarılabilir sürücü")
            };
            let button = div()
                .id(("eject", letter as usize))
                .h(px(30.))
                .px(px(12.))
                .flex()
                .items_center()
                .rounded(px(8.))
                .bg(field)
                .when(busy.is_none(), |d| d.cursor_pointer().hover(move |s| s.bg(hover)).active(move |s| s.bg(pressed)))
                .when(busy.is_some(), |d| d.opacity(0.5))
                .child(t!("Güvenle çıkar"))
                .on_click(cx.listener(move |this, _, _, cx| this.eject(letter, cx)));
            body = body.child(
                h_flex()
                    .h(px(ROW_H))
                    .px(px(8.))
                    .gap(px(10.))
                    .items_center()
                    .child(Icon::new(L::HardDrive).size(px(18.)).flex_shrink_0().text_color(text))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().truncate().child(row_title(letter, &label)))
                            .child(div().text_size(px(11.)).truncate().text_color(if is_failed { danger } else { secondary }).child(under)),
                    )
                    .child(button),
            );
        }
        div()
            .id("eject-panel")
            .relative()
            .size_full()
            .when(grown.is_none(), |d| d.bg(fill))
            .children(grown.map(|g| super::grow_view::wash(&g, fill)))
            .children(grown.and_then(|g| super::grow_view::hairline(&g, look.pal)))
            .text_color(text)
            .text_size(px(13.))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                if this.live() && ev.keystroke.key == "escape" {
                    cx.stop_propagation();
                    cx.defer(|cx| super::panel::close(CloseCause::Escape, cx));
                }
            }))
            .child(match grown {
                Some(g) => super::grow_view::body(&g, body).into_any_element(),
                None => body.into_any_element(),
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    // (explicit names: `super::*` would bring the toolkit's own `test` attribute along and shadow the standard one)
    use super::{EMPTY_H, PAD, ROW_H, WIDTH, row_title, size_for};

    #[test]
    fn the_panel_is_as_high_as_its_rows_and_a_row_names_its_drive() {
        assert_eq!(size_for(0), (WIDTH, 2.0 * PAD + EMPTY_H));
        assert_eq!(size_for(1), (WIDTH, 2.0 * PAD + ROW_H));
        assert_eq!(size_for(3), (WIDTH, 2.0 * PAD + 3.0 * ROW_H));
        assert_eq!(row_title('E', "KINGSTON"), "KINGSTON (E:)");
        assert_eq!(row_title('F', "  "), "F:");
    }
}
