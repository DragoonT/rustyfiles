//! Documents shown without the program that made them: Word, Excel, PowerPoint and
//! OpenDocument files, RTF and HTML as formatted text; SQLite databases, crash dumps and
//! prefetch files as a summary (`rf_win::docs` turns them into Markdown or HTML). Text and
//! structure only: no fonts, pictures or page layout.
//!
//! The file is read on a background thread; "Opening…" shows until it is ready.

use gpui_kit::*;

use crate::look::viewer::{Prepared, Probe, Viewer};
use crate::t;
use rf_win::docs::{Doc, Kind};

#[derive(Default)]
pub struct DocumentViewer {
    view: Option<Entity<DocView>>,
}

struct DocView {
    /// None while the file is being read.
    doc: Option<Result<Doc, String>>,
}

impl Render for DocView {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        use gpui_kit::component::text::TextView;
        let p = crate::theme::Palette::for_window(window);
        let page = div().id("dl-doc-scroll").size_full().overflow_y_scroll().bg(p.content_bg).text_color(p.text).px_8().py_6();
        match &self.doc {
            None => div().size_full().p_6().text_sm().text_color(p.text_secondary).child(t!("Açılıyor…")).into_any_element(),
            Some(Err(e)) => div().size_full().p_4().text_color(p.danger).child(crate::i18n::td(e)).into_any_element(),
            Some(Ok(Doc::Markdown(md))) => page.child(TextView::markdown("dl-doc", SharedString::from(md.clone()))).into_any_element(),
            Some(Ok(Doc::Html(html))) => page.child(TextView::html("dl-doc", SharedString::from(html.clone()))).into_any_element(),
        }
    }
}

fn kind(f: &Probe) -> Option<Kind> {
    if !f.is_file() {
        return None;
    }
    rf_win::docs::kind_of(&f.ext, &f.head)
}

impl Viewer for DocumentViewer {
    fn name(&self) -> &'static str {
        "document"
    }
    fn priority(&self) -> i32 {
        // ahead of the archive viewer (a docx is a zip) and the text viewer (HTML, RTF)
        -4
    }
    fn can_handle(&self, f: &Probe) -> bool {
        kind(f).is_some()
    }
    fn prepare(&mut self, f: &Probe, cx: &mut App) -> Prepared {
        let (path, kind) = (f.path.clone(), kind(f));
        self.view = Some(cx.new(|cx| {
            if let Some(kind) = kind {
                let task = cx.background_spawn(async move { rf_win::docs::read(&path, kind, &|s| crate::i18n::t(s).to_string()).map_err(|e| format!("{e:#}")) });
                cx.spawn(async move |this, cx| {
                    let doc = task.await;
                    let _ = this.update(cx, |v: &mut DocView, cx| {
                        v.doc = Some(doc);
                        cx.notify();
                    });
                })
                .detach();
            }
            DocView { doc: None }
        }));
        Prepared::new((900.0, 760.0), super::file_name(&f.path))
    }
    fn render(&mut self, _window: &mut Window, _cx: &mut App) -> AnyElement {
        match &self.view {
            Some(v) => v.clone().into_any_element(),
            None => div().into_any_element(),
        }
    }
    fn cleanup(&mut self) {
        self.view = None;
    }
}
