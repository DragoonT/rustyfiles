//! The Start panel's detail pane: beside the result list, for the result that is selected. Its picture and name, what kind of thing it
//! is, a few facts (where a file is, how big, when it was changed; an app's maker and version), the things that can be done with it,
//! and for an app the lines of its jump list (its tasks and its recent documents). A child module of `start.rs`: it draws that view's
//! own state and shares its private parts. The rules (which actions, in which order, which keys) are `rf_win::dock::startpane`.
//!
//! HOW IT IS USED. The pane follows the selection: the arrows move it, and the small arrow at the end of a row selects that row
//! without opening it (a click on the row itself opens it, as before). Tab from the field puts the keyboard on the pane's rows: Up and
//! Down move, Enter does, Left, Escape or Shift+Tab lead back. Straight from the list: Enter opens, Ctrl+Enter shows it in its folder,
//! Ctrl+Shift+Enter starts an app as administrator (Windows' own three).
//!
//! WHAT IT COSTS. Nothing until a selection has rested for [`REST`]: then one reading off the UI thread (an app: the shell is asked for
//! its program, the program for its version resource, the jump list is read; a file: one stat). Moving through the list with the
//! arrows starts no reading for the rows passed. A reading that comes back for a row that is no longer selected is dropped.
//!
//! NOTHING JUMPS under the pointer: the rows that depend on a reading keep their place while it runs (a file's size and date show a
//! dash until they are known; an app's "open its folder" is offered until the shell says it has no program), and what arrives late
//! (the jump list) is added below everything else.

use rf_win::dock::startpane::{self as pane, Act, Facts, Kind};

use super::*;
use crate::dock::jump_source::{self, JumpApp};

/// How long a selection rests before its facts are read.
const REST: Duration = Duration::from_millis(90);

/// What is known about the selected result beyond its row.
#[derive(Default)]
pub(super) struct Detail {
    /// The result it is for (`Entry::key`); empty: none.
    pub key: String,
    /// The reading has come back.
    pub loaded: bool,
    /// An app's program, when the shell names one.
    pub program: Option<PathBuf>,
    pub company: Option<String>,
    pub version: Option<String>,
    /// An app's jump list, and what its lines are run with.
    pub jump_app: Option<JumpApp>,
    pub jump: Vec<jump_source::Section>,
    /// A file's size in bytes and a file's or folder's last change (ms since 1970).
    pub size: Option<u64>,
    pub modified_ms: Option<i64>,
}

/// What a reading brings.
#[derive(Default)]
struct Read {
    program: Option<PathBuf>,
    company: Option<String>,
    version: Option<String>,
    jump_app: Option<JumpApp>,
    jump: Vec<jump_source::Section>,
    size: Option<u64>,
    modified_ms: Option<i64>,
}

fn is_program(p: &std::path::Path) -> bool {
    p.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe")) && p.is_file()
}

/// Reads the facts of a result. Blocks (the shell, the disk): a worker thread only.
fn read(entry: &Entry) -> Read {
    match entry {
        Entry::Item(Item { target: Target::App { id, name }, .. }) => {
            let app = AppEntry { id: id.clone(), name: name.clone() };
            let program = startapps::location(&app);
            let exe = program.clone().filter(|p| is_program(p));
            let info = exe.as_deref().and_then(rf_win::pe::info);
            // (the app is named by the panel already: `pinned` keeps that name)
            let jump_app = JumpApp { key: id.to_lowercase(), name: name.clone(), pinned: true, aumid: Some(id.clone()), exe };
            let jump = jump_source::sections(&jump_app);
            Read {
                program,
                company: info.as_ref().and_then(|i| i.company.clone()).filter(|s| !s.trim().is_empty()),
                version: info.and_then(|i| i.product_version.or(i.version)).filter(|s| !s.trim().is_empty()),
                jump_app: Some(jump_app),
                jump,
                ..Default::default()
            }
        }
        Entry::Item(Item { target: Target::Path { path, dir }, .. }) => {
            let meta = std::fs::metadata(path).ok();
            let modified_ms = meta.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64);
            // (a program file says who made it and which version it is, like an app)
            let info = (!dir && is_program(path)).then(|| rf_win::pe::info(path)).flatten();
            Read {
                size: meta.filter(|_| !dir).map(|m| m.len()),
                modified_ms,
                company: info.as_ref().and_then(|i| i.company.clone()).filter(|s| !s.trim().is_empty()),
                version: info.and_then(|i| i.product_version.or(i.version)).filter(|s| !s.trim().is_empty()),
                ..Default::default()
            }
        }
        _ => Read::default(),
    }
}

pub(super) fn kind_of(entry: &Entry) -> Kind {
    match entry {
        Entry::StartMain => Kind::StartMain,
        Entry::Item(item) => match &item.target {
            Target::App { .. } => Kind::App,
            Target::Path { dir: true, .. } => Kind::Folder,
            Target::Path { path, .. } if path.extension().is_some_and(|e| pane::is_program_ext(&e.to_string_lossy().to_lowercase())) => Kind::Program,
            Target::Path { .. } => Kind::File,
            Target::Url { .. } => Kind::Link,
            Target::Web { .. } => Kind::Web,
            Target::Setting { .. } => Kind::Setting,
        },
    }
}

/// An action's picture and words.
fn face(act: Act, kind: Kind) -> (L, &'static str) {
    match act {
        Act::Open => match kind {
            Kind::Web => (L::Search, t!("Webde ara")),
            Kind::StartMain => (L::Play, t!("Başlat")),
            _ => (L::SquareArrowOutUpRight, t!("Aç")),
        },
        Act::Admin => (L::ShieldCheck, t!("Yönetici olarak çalıştır")),
        Act::Location => (L::FolderOpen, t!("Dosya konumunu aç")),
        Act::PinStart => (L::Pin, t!("Başlat'a sabitle")),
        Act::UnpinStart => (L::PinOff, t!("Başlat'tan kaldır")),
        Act::PinDock => (L::Pin, t!("Dock'a sabitle")),
        Act::Uninstall => (L::Trash, t!("Uygulamayı kaldır…")),
        Act::OpenWith => (L::AppWindow, t!("Birlikte aç…")),
        Act::CopyPath => (L::Copy, t!("Yolu kopyala")),
        Act::CopyLink => (L::Link, t!("Bağlantıyı kopyala")),
    }
}

fn web_engine_name(engine: crate::settings::WebSearch) -> &'static str {
    use crate::settings::WebSearch;
    match engine {
        WebSearch::Google => "Google",
        WebSearch::Bing => "Bing",
        WebSearch::DuckDuckGo => "DuckDuckGo",
        WebSearch::Yandex => "Yandex",
    }
}

/// One row of the pane the keyboard can stand on.
#[derive(Clone)]
enum PaneRow {
    Act(Act),
    Jump(jump_source::Action),
}

impl StartView {
    /// The pane follows the selection: when another result is selected its facts are forgotten, and read once the selection has rested.
    fn sync_detail(&mut self, cx: &mut Context<Self>) {
        let entry = self.entries.get(self.sel_ix).cloned();
        let key = entry.as_ref().map(Entry::key).unwrap_or_default();
        if key == self.detail.key {
            return;
        }
        self.detail = Detail { key: key.clone(), ..Default::default() };
        self.pane_cursor = None;
        self._detail = None;
        let Some(entry) = entry else { return };
        // (nothing to read for a link, a web search, a settings page: they are "loaded" as they are)
        if !matches!(kind_of(&entry), Kind::App | Kind::Program | Kind::File | Kind::Folder) {
            self.detail.loaded = true;
            return;
        }
        self._detail = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(REST).await;
            let got = cx.background_executor().spawn(async move { read(&entry) }).await;
            let _ = this.update(cx, |me, cx| {
                if me.detail.key != key {
                    return;
                }
                me.detail = Detail { key, loaded: true, program: got.program, company: got.company, version: got.version, jump_app: got.jump_app, jump: got.jump, size: got.size, modified_ms: got.modified_ms };
                cx.notify();
            });
        }));
    }

    fn pane_facts(&self, entry: &Entry) -> Facts {
        match entry {
            Entry::Item(Item { target: Target::App { id, .. }, .. }) => Facts {
                // (offered until the shell has said that the app has no program: see the top of this file)
                has_program: !self.detail.loaded || self.detail.program.is_some(),
                pinned_start: self.pins.is_pinned(id),
            },
            _ => Facts::default(),
        }
    }

    /// The jump list's lines that are shown: each section's title (on its first line only) and its lines, the tasks first, cut to a
    /// glance.
    fn pane_jump(&self) -> Vec<(Option<String>, jump_source::Entry)> {
        let tasks = t!("Görevler");
        let tasks_from = self.detail.jump.iter().position(|s| s.title.as_deref() == Some(tasks));
        let sections: Vec<&jump_source::Section> = pane::jump_order(self.detail.jump.len(), tasks_from).into_iter().map(|i| &self.detail.jump[i]).collect();
        let lens: Vec<usize> = sections.iter().map(|s| s.entries.len()).collect();
        let mut out = Vec::new();
        for (section, take) in sections.into_iter().zip(pane::jump_shown(&lens, pane::JUMP_MAX)) {
            for (i, e) in section.entries.iter().take(take).enumerate() {
                out.push((if i == 0 { section.title.clone() } else { None }, e.clone()));
            }
        }
        out
    }

    /// The rows the keyboard goes through: the actions, then the jump list's lines.
    fn pane_rows(&self) -> Vec<PaneRow> {
        let Some(entry) = self.entries.get(self.sel_ix) else { return Vec::new() };
        let mut rows: Vec<PaneRow> = pane::actions(kind_of(entry), self.pane_facts(entry)).into_iter().map(PaneRow::Act).collect();
        rows.extend(self.pane_jump().into_iter().map(|(_, e)| PaneRow::Jump(e.action)));
        rows
    }

    /// A key while results are shown: the pane's part of it. True when the key was the pane's.
    pub(super) fn pane_key(&mut self, key: &str, m: Modifiers, in_field: bool, cx: &mut Context<Self>) -> bool {
        match self.pane_cursor {
            None => match key {
                "tab" if !m.shift && in_field && !self.pane_rows().is_empty() => {
                    self.pane_cursor = Some(0);
                    cx.notify();
                    true
                }
                // (Windows' own: Ctrl+Shift+Enter starts the selected app as administrator)
                "enter" if pane::enter_act(m.control, m.shift) == Act::Admin => {
                    self.pane_do(Act::Admin, cx);
                    true
                }
                _ => false,
            },
            Some(at) => match key {
                "up" | "down" => {
                    self.pane_cursor = Some(pane::step(at, key == "up", self.pane_rows().len()));
                    cx.notify();
                    true
                }
                "enter" => {
                    let rows = self.pane_rows();
                    match rows.get(at.min(rows.len().saturating_sub(1))).cloned() {
                        Some(PaneRow::Act(act)) => self.pane_do(act, cx),
                        Some(PaneRow::Jump(action)) => self.pane_run_jump(action, cx),
                        None => {}
                    }
                    true
                }
                "escape" | "left" => {
                    self.pane_cursor = None;
                    cx.notify();
                    true
                }
                "tab" => {
                    self.pane_cursor = None;
                    cx.notify();
                    // (Shift+Tab is the way back to the list; Tab goes on to the panel's next stop)
                    m.shift
                }
                _ => false,
            },
        }
    }

    /// An action of the pane was chosen, for the selected result.
    pub(super) fn pane_do(&mut self, act: Act, cx: &mut Context<Self>) {
        if !self.live() {
            return;
        }
        let Some(entry) = self.entries.get(self.sel_ix).cloned() else { return };
        if !pane::actions(kind_of(&entry), self.pane_facts(&entry)).contains(&act) {
            return;
        }
        let app = match &entry {
            Entry::Item(Item { target: Target::App { id, name }, .. }) => Some(AppEntry { id: id.clone(), name: name.clone() }),
            _ => None,
        };
        let path = match &entry {
            Entry::Item(Item { target: Target::Path { path, .. }, .. }) => Some(path.clone()),
            _ => None,
        };
        match act {
            Act::Open => self.activate(How::Open, cx),
            Act::Location => self.activate(How::ShowInFolder, cx),
            Act::Admin => {
                if let Some(app) = app {
                    self.launch_app_as_admin(app, cx);
                } else if let Some(path) = path {
                    // Windows shows its consent prompt: the user asked for it with this choice
                    self.run_then_close(crate::shellless::start_failure_text, move || startapps::launch_file_as_admin(&path), cx);
                }
            }
            Act::PinStart | Act::UnpinStart => {
                if let Some(app) = app {
                    self.toggle_pin(&app, cx);
                }
            }
            Act::PinDock => {
                if let Some(app) = app {
                    crate::dock::own_pins::pin_start_app(app, cx);
                    self.say(t!("Dock'a sabitlendi"), cx);
                }
            }
            Act::Uninstall => {
                // Windows' own list of installed apps: the app is removed there (no program is run for it from here)
                crate::dock::panel_parts::open_uri("ms-settings:appsfeatures");
                self.finish(cx);
            }
            Act::OpenWith => {
                if let Some(path) = path {
                    Self::let_it_come_forward();
                    std::thread::spawn(move || {
                        if rf_win::sta::call(Duration::from_secs(15), move || rf_win::shell::open_with(&path)).and_then(Result::ok).is_none() {
                            log::warn!("start: the open-with dialog could not be shown");
                        }
                    });
                    self.finish(cx);
                }
            }
            Act::CopyPath => {
                if let Some(path) = path {
                    cx.write_to_clipboard(ClipboardItem::new_string(path.display().to_string()));
                    self.say(t!("Yol kopyalandı"), cx);
                }
            }
            Act::CopyLink => {
                if let Entry::Item(Item { target: Target::Url { url }, .. }) = &entry {
                    cx.write_to_clipboard(ClipboardItem::new_string(url.clone()));
                    self.say(t!("Bağlantı kopyalandı"), cx);
                }
            }
        }
        cx.notify();
    }

    /// A line of the app's jump list was chosen.
    fn pane_run_jump(&mut self, action: jump_source::Action, cx: &mut Context<Self>) {
        if !self.live() {
            return;
        }
        let Some(app) = self.detail.jump_app.clone() else { return };
        self.run_then_close(crate::shellless::start_failure_text, move || jump_source::run(&app, &action), cx);
    }

    /// The head's picture, the title and the line under it.
    fn pane_head(&mut self, entry: &Entry, pal: Palette, cx: &mut Context<Self>) -> (SharedString, AnyElement, Option<String>) {
        let secondary = pal.text_secondary().hsla();
        let glyph = |icon: L| {
            div().size(px(pane::ICON)).rounded(px(14.)).bg(pal.placeholder_fill().hsla()).flex().items_center().justify_center().child(Icon::new(icon).size(px(26.)).text_color(secondary)).into_any_element()
        };
        match entry {
            Entry::StartMain => (
                t!("Dosyaları aramak için rustyfiles'ı arka planda başlat").into(),
                glyph(L::HardDrive),
                Some(t!("Dosya dizini ana uygulamada tutulur; 10 dakika aranmazsa kendiliğinden kapanır").to_string()),
            ),
            Entry::Item(item) => match &item.target {
                Target::App { id, name } => {
                    let picture = self.app_picture(&AppEntry { id: id.clone(), name: name.clone() }, pane::ICON, pal, cx);
                    let mut sub = vec![t!("Uygulama").to_string()];
                    sub.extend(self.detail.company.clone());
                    sub.extend(self.detail.version.clone());
                    (item.title.clone().into(), picture, Some(sub.join(" · ")))
                }
                Target::Path { path, dir } => {
                    let picture = self.picture(format!("file:{}", path.to_string_lossy().to_lowercase()), IconSource::File(path.clone()), &item.title, pane::ICON, pal, cx);
                    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
                    let mut sub = vec![crate::icons::type_label_of(*dir, &ext)];
                    sub.extend(self.detail.company.clone());
                    sub.extend(self.detail.version.clone());
                    (item.title.clone().into(), picture, Some(sub.join(" · ")))
                }
                Target::Url { url } => (item.title.clone().into(), glyph(L::Globe), Some(url.clone())),
                Target::Web { query } => (
                    format!("\u{201c}{query}\u{201d} {}", t!("için webde ara")).into(),
                    glyph(L::Search),
                    Some(web_engine_name(cx.global::<Settings>().web_search).to_string()),
                ),
                Target::Setting { .. } => (item.title.clone().into(), glyph(L::Settings), (!item.detail.is_empty()).then(|| item.detail.clone())),
            },
        }
    }

    /// The facts under the head, as label and value. A value that is still being read is a dash: its row keeps its place.
    fn pane_info(&self, entry: &Entry) -> Vec<(&'static str, String)> {
        let Entry::Item(Item { target: Target::Path { path, dir }, .. }) = entry else { return Vec::new() };
        let pending = |v: Option<String>| v.unwrap_or_else(|| "\u{2013}".to_string());
        let mut rows = vec![(t!("Konum"), path.parent().map(|p| p.display().to_string()).unwrap_or_default())];
        if !dir {
            rows.push((t!("Boyut"), pending(self.detail.size.map(rf_core::format::size))));
        }
        rows.push((t!("Değiştirme tarihi"), pending(self.detail.modified_ms.map(rf_core::format::datetime))));
        rows
    }

    /// The pane, for the selected result.
    pub(super) fn render_pane(&mut self, pal: Palette, theme: ThemePalette, cx: &mut Context<Self>) -> AnyElement {
        self.sync_detail(cx);
        let Some(entry) = self.entries.get(self.sel_ix).cloned() else { return div().into_any_element() };
        let kind = kind_of(&entry);
        let (secondary, hover, sep, sel_fill) = (pal.text_secondary().hsla(), pal.hover_fill().hsla(), pal.separator().hsla(), theme.selection);
        let (title, picture, sub) = self.pane_head(&entry, pal, cx);
        let acts = pane::actions(kind, self.pane_facts(&entry));
        let cursor = self.pane_cursor;

        let mut col = v_flex().w_full().px(px(14.)).pt(px(20.)).pb(px(12.));
        col = col.child(
            v_flex()
                .items_center()
                .gap(px(4.))
                .pb(px(14.))
                .child(div().size(px(pane::ICON)).mb(px(6.)).flex().items_center().justify_center().child(picture))
                .child(div().w_full().px(px(6.)).text_center().text_base().font_weight(FontWeight::SEMIBOLD).line_clamp(2).child(title))
                .when_some(sub, |d, s| d.child(div().w_full().px(px(6.)).text_center().text_xs().text_color(secondary).line_clamp(3).child(s))),
        );
        let info = self.pane_info(&entry);
        if !info.is_empty() {
            let mut block = v_flex().w_full().px(px(10.)).pb(px(10.)).gap(px(3.));
            for (label, value) in info {
                block = block.child(
                    h_flex()
                        .w_full()
                        .gap(px(8.))
                        .items_start()
                        .text_xs()
                        .child(div().w(px(104.)).flex_shrink_0().text_color(secondary).child(label))
                        .child(div().flex_1().min_w_0().line_clamp(2).child(value)),
                );
            }
            col = col.child(block);
        }
        for (i, act) in acts.iter().copied().enumerate() {
            if pane::starts_group(&acts, i) {
                col = col.child(div().mx(px(10.)).my(px(5.)).h(px(1.)).bg(sep));
            }
            let (icon, label) = face(act, kind);
            let on = cursor == Some(i);
            col = col.child(
                h_flex()
                    .id(("start-pane-act", i))
                    .h(px(pane::ROW_H))
                    .px(px(10.))
                    .gap(px(10.))
                    .items_center()
                    .rounded(px(8.))
                    .cursor_pointer()
                    .when(on, |d| d.bg(sel_fill))
                    // (opening is the first thing asked for: its row carries a plate of its own, so the eye finds it)
                    .when(!on && i == 0, |d| d.bg(hover))
                    .when(!on, |d| d.hover(move |s| s.bg(sel_fill)))
                    .child(Icon::new(icon).size(px(16.)).flex_shrink_0().text_color(if i == 0 { pal.text().hsla() } else { secondary }))
                    .child(div().flex_1().min_w_0().text_sm().truncate().when(i == 0, |d| d.font_weight(FontWeight::MEDIUM)).child(label))
                    .when_some(pane::shortcut(act), |d, keys| d.child(div().flex_shrink_0().text_xs().text_color(secondary).child(keys)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.pane_do(act, cx))),
            );
        }
        // an app's own jump list: its tasks and what it opened lately
        let mut row = acts.len();
        for (heading, e) in self.pane_jump() {
            if let Some(h) = heading {
                col = col.child(div().px(px(10.)).pt(px(14.)).pb(px(4.)).text_xs().text_color(secondary).child(h));
            }
            let on = cursor == Some(row);
            let picture = match &e.icon {
                Some(src) => self.picture(format!("jump:{}", e.action.0.to_lowercase()), src.clone(), &e.title, 18.0, pal, cx),
                None => Icon::new(L::File).size(px(16.)).text_color(secondary).into_any_element(),
            };
            let action = e.action.clone();
            col = col.child(
                h_flex()
                    .id(("start-pane-jump", row))
                    .h(px(pane::ROW_H - 2.0))
                    .px(px(10.))
                    .gap(px(10.))
                    .items_center()
                    .rounded(px(8.))
                    .cursor_pointer()
                    .when(on, |d| d.bg(sel_fill))
                    .when(!on, |d| d.hover(move |s| s.bg(hover)))
                    .child(div().size(px(18.)).flex_shrink_0().flex().items_center().justify_center().child(picture))
                    .child(div().flex_1().min_w_0().text_sm().truncate().child(e.title.clone()))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.pane_run_jump(action.clone(), cx))),
            );
            row += 1;
        }
        div().id("start-pane").size_full().overflow_y_scroll().child(col).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{Entry, Item, Kind, Section, Target, face, kind_of, pane};
    use std::path::PathBuf;

    fn item(target: Target) -> Entry {
        Entry::Item(Item { target, title: "x".into(), detail: String::new(), section: Section::Apps, score: 0 })
    }

    #[test]
    fn every_result_has_its_kind() {
        assert_eq!(kind_of(&item(Target::App { id: "a".into(), name: "A".into() })), Kind::App);
        assert_eq!(kind_of(&item(Target::Path { path: PathBuf::from(r"C:\x\y.txt"), dir: false })), Kind::File);
        assert_eq!(kind_of(&item(Target::Path { path: PathBuf::from(r"C:\Windows\System32\CMD.EXE"), dir: false })), Kind::Program);
        assert_eq!(kind_of(&item(Target::Path { path: PathBuf::from(r"C:\x\tool.exe"), dir: true })), Kind::Folder, "a folder named like a program");
        assert_eq!(kind_of(&item(Target::Path { path: PathBuf::from(r"C:\x"), dir: true })), Kind::Folder);
        assert_eq!(kind_of(&item(Target::Url { url: "https://example.com".into() })), Kind::Link);
        assert_eq!(kind_of(&item(Target::Web { query: "q".into() })), Kind::Web);
        assert_eq!(kind_of(&item(Target::Setting { launch: "ms-settings:display".into(), name: "Ekran".into() })), Kind::Setting);
        assert_eq!(kind_of(&Entry::StartMain), Kind::StartMain);
    }

    /// Every action of every kind has words and a picture, and no two actions of one kind read the same.
    #[test]
    fn the_actions_of_a_kind_can_be_told_apart() {
        crate::dock_page::while_turkish(|| {
            for kind in [Kind::App, Kind::Program, Kind::File, Kind::Folder, Kind::Link, Kind::Web, Kind::Setting, Kind::StartMain] {
                for f in [pane::Facts::default(), pane::Facts { has_program: true, pinned_start: true }] {
                    let acts = pane::actions(kind, f);
                    let labels: std::collections::HashSet<&str> = acts.iter().map(|a| face(*a, kind).1).collect();
                    assert_eq!(labels.len(), acts.len(), "{kind:?}");
                    assert!(labels.iter().all(|l| !l.trim().is_empty()));
                }
            }
            // opening a web search and starting the main app say what they do
            assert_ne!(face(pane::Act::Open, Kind::Web).1, face(pane::Act::Open, Kind::App).1);
            assert_ne!(face(pane::Act::Open, Kind::StartMain).1, face(pane::Act::Open, Kind::App).1);
        });
    }
}
