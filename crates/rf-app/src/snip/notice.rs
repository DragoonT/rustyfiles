//! The quiet notice after a snip, in the dock's style: a small panel above the bottom of the work area that says "Panoya kopyalandı"
//! (or what else became of it) and offers "Kaydet", which writes the PNG to `Pictures\Screenshots`. It goes after six seconds unless
//! the pointer is on it or a save is going on. It never takes the foreground, so the program the user was in keeps the keyboard.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName as L;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::finish::{self, Made};
use super::plan::{self, Headline, Outcome};
use crate::dock::look::Palette;
use crate::dock::panel_parts::Look;
use crate::glass_watch::{self, PopupGlass};
use crate::t;

pub struct NoticeView {
    outcome: Outcome,
    /// The PNG `Kaydet` writes (the bytes the clipboard got).
    png: Option<Arc<Vec<u8>>>,
    hwnd: isize,
    dark: bool,
    popup: Option<PopupGlass>,
    _watch: Option<Task<()>>,
    /// When the notice was shown, or when its last save ended: the six seconds count from here.
    started: Instant,
    hovered: bool,
    saving: bool,
    _tasks: Vec<Task<()>>,
}

/// The first line.
fn headline_text(h: Headline) -> &'static str {
    match h {
        Headline::Copied => t!("Panoya kopyalandı"),
        Headline::CopiedAndSaved => t!("Panoya kopyalandı ve kaydedildi"),
        Headline::SavedOnly => t!("Kaydedildi"),
        Headline::NotCopied => t!("Panoya kopyalanamadı"),
        Headline::Failed => t!("Alıntı alınamadı"),
    }
}

/// The second line: the file's name once it is saved, a failed save, or else the size of the picture.
fn subtitle(o: &Outcome, saving: bool) -> String {
    if saving {
        return t!("Kaydediliyor…").to_string();
    }
    match (&o.saved, o.size) {
        (Some(Ok(path)), _) => path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        (Some(Err(_)), _) => t!("Dosya kaydedilemedi").to_string(),
        (None, Some((w, h))) => plan::size_text(w as i32, h as i32),
        (None, None) => String::new(),
    }
}

impl NoticeView {
    pub fn new(made: Made, window: &mut Window, _cx: &mut Context<Self>) -> Self {
        NoticeView {
            outcome: made.outcome,
            png: made.png,
            hwnd: crate::look::window::hwnd_of(window),
            dark: rf_win::accent::system_is_dark(),
            popup: None,
            _watch: None,
            started: Instant::now(),
            hovered: false,
            saving: false,
            _tasks: Vec::new(),
        }
    }

    /// The window is placed and shown: the glass goes behind it and the clock starts.
    pub fn begin(&mut self, cx: &mut Context<Self>) {
        self.started = Instant::now();
        if self.popup.is_none() && self.hwnd != 0 {
            crate::glass::keep_out_of_capture(self.hwnd);
            self.popup = Some(PopupGlass::start_panel(self.hwnd, self.dark));
            self._watch = Some(glass_watch::watch(cx, |t: &mut NoticeView| t.popup.as_mut()));
        }
        let timer = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(200)).await;
                let Ok(over) = this.update(cx, |t, _| plan::notice_expired(t.started.elapsed().as_millis() as u64, t.hovered, t.saving)) else { break };
                if over {
                    cx.update(super::close_notice);
                    break;
                }
            }
        });
        self._tasks.push(timer);
        cx.notify();
    }

    pub fn release_glass(&mut self) {
        self._watch = None;
        if let Some(mut g) = self.popup.take() {
            g.release();
        }
    }

    /// `Kaydet`: the PNG is written on a thread of its own.
    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(png) = self.png.clone() else { return };
        if self.saving {
            return;
        }
        self.saving = true;
        cx.notify();
        // The write counts as work of the process from here to its end, whatever becomes of this window: a `--snip` process whose notice
        // is closed during the write must not end under it and leave half a file.
        super::work_begin();
        let (tx, rx) = futures::channel::oneshot::channel();
        let spawned = std::thread::Builder::new().name("rf-snip-save".into()).spawn(move || {
            let _ = tx.send(finish::save_now(&png));
        });
        if spawned.is_err() {
            self.saving = false;
            self.outcome.saved = Some(Err("the save thread could not be started".into()));
            cx.defer(super::work_end);
            return;
        }
        // (detached, not kept in `_tasks`: it must run to its end after the window has gone, to give the work back)
        cx.spawn(async move |this, cx| {
            let result = rx.await.unwrap_or_else(|_| Err("the save thread ended".to_string()));
            if let Err(e) = &result {
                log::warn!("snip: the file could not be saved: {e}");
            }
            let _ = this.update(cx, |t, cx| {
                t.saving = false;
                t.started = Instant::now();
                t.outcome.saved = Some(result);
                cx.notify();
            });
            cx.update(super::work_end);
        })
        .detach();
    }
}

impl Render for NoticeView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(Palette { mix: if self.dark { 0.0 } else { 1.0 } });
        let fill = glass_watch::panel_fill(self.dark, self.popup.as_ref().is_none_or(|g| g.fallback()));
        let (hover, pressed) = (look.pal.hover_fill().hsla(), look.pal.pressed_fill().hsla());
        let headline = plan::headline(&self.outcome);
        let good = !matches!(headline, Headline::NotCopied | Headline::Failed);
        let offer = plan::offers_save(&self.outcome) && !self.saving;
        let mark = if good { L::Check } else { L::X };
        h_flex()
            .id("snip-notice")
            .size_full()
            .bg(fill)
            .px(px(12.))
            .gap(px(10.))
            .items_center()
            .text_color(look.pal.text().hsla())
            .on_hover(cx.listener(|this, on: &bool, _, _| this.hovered = *on))
            .child(
                div()
                    .size(px(30.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(if good { look.accent } else { look.pal.field_fill().hsla() })
                    .child(Icon::new(mark).size(px(16.)).text_color(if good { look.on_accent } else { look.pal.text().hsla() })),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().truncate().text_size(px(13.5)).font_weight(FontWeight::SEMIBOLD).child(headline_text(headline)))
                    .child(div().truncate().text_size(px(12.)).text_color(look.pal.text_secondary().hsla()).child(subtitle(&self.outcome, self.saving))),
            )
            .when(offer, |d| {
                d.child(
                    div()
                        .id("snip-save")
                        .h(px(30.))
                        .px(px(12.))
                        .flex()
                        .items_center()
                        .rounded(px(7.))
                        .bg(look.pal.field_fill().hsla())
                        .cursor_pointer()
                        .hover(move |s| s.bg(hover))
                        .active(move |s| s.bg(pressed))
                        .text_size(px(13.))
                        .child(t!("Kaydet"))
                        .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                )
            })
            .child(
                div()
                    .id("snip-notice-close")
                    .size(px(28.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .active(move |s| s.bg(pressed))
                    .child(Icon::new(L::X).size(px(14.)).text_color(look.pal.text_secondary().hsla()))
                    .on_click(cx.listener(|_, _, _, cx| cx.defer(super::close_notice))),
            )
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use rf_win::snip::clip::Written;

    // (explicit names: `use super::*` would bring in the toolkit's own `test` attribute through its glob and break `#[test]`)
    use super::{Headline, Outcome, headline_text, subtitle};

    fn outcome(saved: Option<Result<PathBuf, String>>) -> Outcome {
        Outcome { size: Some((1280, 720)), copied: Ok(Written::All), saved, has_png: true }
    }

    #[test]
    fn the_second_line_is_the_size_then_the_file_name_and_says_when_a_save_failed_or_is_going_on() {
        crate::dock_page::while_turkish(|| {
            assert_eq!(subtitle(&outcome(None), false), "1280 \u{d7} 720");
            let saved = outcome(Some(Ok(PathBuf::from("C:\\x\\Ekran alıntısı 2026-11-13 030405.png"))));
            assert_eq!(subtitle(&saved, false), "Ekran alıntısı 2026-11-13 030405.png");
            assert_eq!(subtitle(&outcome(Some(Err("disk".into()))), false), "Dosya kaydedilemedi");
            assert_eq!(subtitle(&outcome(None), true), "Kaydediliyor…");
            let none = Outcome { size: None, copied: Err("x".into()), saved: None, has_png: false };
            assert_eq!(subtitle(&none, false), "");
        });
    }

    #[test]
    fn every_headline_has_its_words() {
        crate::dock_page::while_turkish(|| {
            assert_eq!(headline_text(Headline::Copied), "Panoya kopyalandı");
            assert_eq!(headline_text(Headline::CopiedAndSaved), "Panoya kopyalandı ve kaydedildi");
            assert_eq!(headline_text(Headline::SavedOnly), "Kaydedildi");
            assert_eq!(headline_text(Headline::NotCopied), "Panoya kopyalanamadı");
            assert_eq!(headline_text(Headline::Failed), "Alıntı alınamadı");
        });
    }
}
