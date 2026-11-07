//! Our own Properties window (Files has one instead of the Win32 property sheet): General
//! (name, type, default app, location, size / size on disk / contents, times, read-only and
//! hidden), Details (media / document properties) and Hashes. "Windows properties…" still opens
//! the shell sheet for security, sharing and previous versions.
//!
//! Every disk access happens on background threads; the window never waits on a slow drive.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::assets::IconName as L;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{Icon, TitleBar, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rf_core::{Entry, EntryKind, format};
use rf_win::fileinfo::Info;

use crate::image_cache::ImageCache;
use crate::t;
use crate::theme::{self, Palette};

const READONLY: u32 = 0x1;
const HIDDEN: u32 = 0x2;

/// Opens a Properties window for `paths` (one window per call).
pub fn open(paths: Vec<PathBuf>, images: Entity<ImageCache>, cx: &mut App) {
    if paths.is_empty() {
        return;
    }
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(size(px(460.), px(640.)), cx)),
        window_min_size: Some(size(px(380.), px(440.))),
        window_background: WindowBackgroundAppearance::MicaBackdrop,
        ..TitleBar::window_options()
    };
    if let Err(e) = gpui_kit::open_window(options, cx, move |window, cx| cx.new(|cx| PropsView::new(paths, images, window, cx))) {
        log::error!("properties window: {e:#}");
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Page {
    General,
    Details,
    Hashes,
}

/// Totals for a multi-selection.
#[derive(Default, Clone)]
struct Totals {
    files: u64,
    dirs: u64,
    bytes: u64,
    done: bool,
}

pub struct PropsView {
    paths: Vec<PathBuf>,
    images: Entity<ImageCache>,
    page: Page,
    info: Option<Result<Info, String>>,
    folder: Option<Option<rf_win::foldersize::FolderSize>>,
    totals: Option<Totals>,
    media: Vec<(&'static str, String)>,
    /// .lnk: (target, arguments, start in)
    shortcut: Option<(String, String, String)>,
    name: Option<Entity<InputState>>,
    /// Checkbox states (None = mixed across a multi-selection) and what they were on load.
    readonly: Option<bool>,
    hidden: Option<bool>,
    loaded_flags: (Option<bool>, Option<bool>),
    hash: Option<Entity<crate::hash_panel::HashPanel>>,
    error: Option<SharedString>,
    busy: bool,
    frame_dark: Option<bool>,
    cancel: Arc<AtomicBool>,
    focus: FocusHandle,
}

impl Drop for PropsView {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.to_string_lossy().into_owned())
}

impl PropsView {
    fn new(paths: Vec<PathBuf>, images: Entity<ImageCache>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe(&images, |_, _, cx| cx.notify()).detach();
        let single = paths.len() == 1;
        let name = single.then(|| {
            let n = file_name(&paths[0]);
            cx.new(|cx| {
                let mut s = InputState::new(window, cx);
                s.set_value(n, window, cx);
                s
            })
        });
        let mut v = Self {
            paths,
            images,
            page: Page::General,
            info: None,
            folder: None,
            totals: None,
            media: Vec::new(),
            shortcut: None,
            name,
            readonly: None,
            hidden: None,
            loaded_flags: (None, None),
            hash: None,
            error: None,
            busy: false,
            frame_dark: None,
            cancel: Default::default(),
            focus: cx.focus_handle(),
        };
        v.load(cx);
        window.focus(&v.focus, cx);
        window.activate_window();
        v
    }

    fn single(&self) -> Option<&Path> {
        (self.paths.len() == 1).then(|| self.paths[0].as_path())
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let cancel = self.cancel.clone();
        if let Some(path) = self.single().map(Path::to_path_buf) {
            let p = path.clone();
            let task = cx.background_spawn(async move { rf_win::fileinfo::info(&p).map_err(|e| e.to_string()) });
            cx.spawn(async move |this, cx| {
                let r = task.await;
                let _ = this.update(cx, |v, cx| {
                    if let Ok(i) = &r {
                        let flags = (Some(i.attrs & READONLY != 0), Some(i.attrs & HIDDEN != 0));
                        (v.readonly, v.hidden) = flags;
                        v.loaded_flags = flags;
                        v.start_details(i.is_dir, cx);
                    }
                    v.info = Some(r);
                    cx.notify();
                });
            })
            .detach();
        } else {
            self.totals = Some(Totals::default());
            let paths = self.paths.clone();
            let (tx, mut rx) = futures::channel::mpsc::unbounded::<(Totals, Option<bool>, Option<bool>)>();
            std::thread::Builder::new()
                .name("rf-props".into())
                .spawn(move || {
                    let cache = crate::preview::size_cache();
                    let mut t = Totals::default();
                    let (mut ro, mut hid): (Option<bool>, Option<bool>) = (None, None);
                    let merge = |acc: &mut Option<bool>, v: bool, first: bool| {
                        *acc = if first { Some(v) } else if *acc == Some(v) { *acc } else { None };
                    };
                    for (i, p) in paths.iter().enumerate() {
                        if cancel.load(Ordering::Relaxed) {
                            return;
                        }
                        let Ok(info) = rf_win::fileinfo::info(p) else { continue };
                        merge(&mut ro, info.attrs & READONLY != 0, i == 0);
                        merge(&mut hid, info.attrs & HIDDEN != 0, i == 0);
                        if info.is_dir {
                            t.dirs += 1;
                            if let Ok(fs) = cache.get_or_compute(p, &|| cancel.load(Ordering::Relaxed), &mut |_| {}) {
                                t.bytes += fs.bytes;
                                t.files += fs.files;
                                t.dirs += fs.dirs;
                            }
                        } else {
                            t.files += 1;
                            t.bytes += info.size;
                        }
                        let _ = tx.unbounded_send((t.clone(), ro, hid));
                    }
                    t.done = true;
                    let _ = tx.unbounded_send((t, ro, hid));
                })
                .ok();
            cx.spawn(async move |this, cx| {
                use futures::StreamExt;
                let mut first = true;
                while let Some((t, ro, hid)) = rx.next().await {
                    let done = t.done;
                    let ok = this
                        .update(cx, |v, cx| {
                            v.totals = Some(t);
                            if first || done {
                                (v.readonly, v.hidden) = (ro, hid);
                                v.loaded_flags = (ro, hid);
                            }
                            cx.notify();
                        })
                        .is_ok();
                    first = false;
                    if !ok {
                        break;
                    }
                }
            })
            .detach();
        }
    }

    /// Folder contents (folders) or media properties (files), after the basic info arrived.
    fn start_details(&mut self, is_dir: bool, cx: &mut Context<Self>) {
        let Some(path) = self.single().map(Path::to_path_buf) else { return };
        let cancel = self.cancel.clone();
        if is_dir {
            self.folder = Some(None);
            let task = cx.background_spawn(async move {
                crate::preview::size_cache().get_or_compute(&path, &|| cancel.load(Ordering::Relaxed), &mut |_| {}).ok()
            });
            cx.spawn(async move |this, cx| {
                let r = task.await;
                let _ = this.update(cx, |v, cx| {
                    v.folder = Some(r);
                    cx.notify();
                });
            })
            .detach();
        } else {
            if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("lnk")) {
                let p = path.clone();
                let task = cx.background_spawn(async move { rf_win::shortcut::resolve(&p).ok() });
                cx.spawn(async move |this, cx| {
                    let r = task.await;
                    let _ = this.update(cx, |v, cx| {
                        v.shortcut = r.map(|s| {
                            (s.target.to_string_lossy().into_owned(), s.args, s.working_dir.to_string_lossy().into_owned())
                        });
                        cx.notify();
                    });
                })
                .detach();
            }
            let task = cx.background_spawn(async move { rf_win::props::media_props(&path) });
            cx.spawn(async move |this, cx| {
                let r = task.await;
                let _ = this.update(cx, |v, cx| {
                    v.media = r;
                    cx.notify();
                });
            })
            .detach();
        }
    }

    fn dirty(&self, cx: &App) -> bool {
        let renamed = match (&self.name, self.single()) {
            (Some(n), Some(p)) => n.read(cx).value().trim() != file_name(p),
            _ => false,
        };
        renamed || (self.readonly, self.hidden) != self.loaded_flags
    }

    /// Applies the edits on a helper thread; `close` closes the window when it worked.
    fn apply(&mut self, close: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if !self.dirty(cx) {
            if close {
                window.remove_window();
            }
            return;
        }
        let new_name = match (&self.name, self.single()) {
            (Some(n), Some(p)) => {
                let v = n.read(cx).value().trim().to_string();
                (v != file_name(p)).then_some(v)
            }
            _ => None,
        };
        if let Some(n) = &new_name {
            if let Err(e) = rf_core::rename::validate(n) {
                self.error = Some(crate::i18n::td(&e.to_string()).into());
                cx.notify();
                return;
            }
        }
        let flags = (
            (self.readonly != self.loaded_flags.0).then_some(self.readonly).flatten(),
            (self.hidden != self.loaded_flags.1).then_some(self.hidden).flatten(),
        );
        let paths = self.paths.clone();
        self.busy = true;
        self.error = None;
        let task = cx.background_spawn(async move {
            let mut paths = paths;
            if flags.0.is_some() || flags.1.is_some() {
                for p in &paths {
                    rf_win::fileinfo::set_flags(p, flags.0, flags.1).map_err(|e| format!("{}: {e}", file_name(p)))?;
                }
            }
            let mut renamed = None;
            if let Some(n) = new_name {
                let from = paths[0].clone();
                let to = from.with_file_name(&n);
                if to.exists() && !n.eq_ignore_ascii_case(&file_name(&from)) {
                    return Err(t!("Bu adla bir öğe zaten var").to_string());
                }
                std::fs::rename(&from, &to).map_err(|e| e.to_string())?;
                renamed = Some((from, to.clone()));
                paths[0] = to;
            }
            Ok::<_, String>((paths, renamed))
        });
        cx.spawn_in(window, async move |this, cx| {
            let r = task.await;
            let _ = this.update_in(cx, |v, window, cx| {
                v.busy = false;
                match r {
                    Ok((paths, renamed)) => {
                        if let Some((from, to)) = renamed {
                            crate::undo::push(crate::undo::UndoOp::Renamed { from, to });
                        }
                        v.paths = paths;
                        v.loaded_flags = (v.readonly, v.hidden);
                        if close {
                            window.remove_window();
                        }
                    }
                    Err(e) => v.error = Some(e.into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn windows_properties(&self) {
        let p = self.paths[0].clone();
        rf_win::sta::run(move |_| {
            let _ = rf_win::shell::properties(&p);
        });
    }

    fn entry(&self) -> Option<Entry> {
        let p = self.single()?;
        let Some(Ok(i)) = &self.info else { return None };
        let kind = if i.is_dir { EntryKind::Dir } else { EntryKind::File };
        Some(Entry::new(&file_name(p), kind, i.size, i.modified_ms, i.created_ms, i.attrs))
    }

    // ---------- rendering ----------

    fn render_general(&self, p: &Palette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = Vec::new();
        let mut card1 = Vec::new();
        if let Some(path) = self.single() {
            match &self.info {
                None => card1.push(row(p, t!("Durum"), t!("Yükleniyor…").into())),
                Some(Err(e)) => card1.push(row(p, t!("Hata"), e.clone())),
                Some(Ok(i)) => {
                    if let Some(e) = self.entry() {
                        card1.push(row(p, t!("Tür"), crate::icons::type_label(&e).to_string()));
                    }
                    if let Some(app) = &i.opens_with {
                        card1.push(row(p, t!("Birlikte açılır"), app.clone()));
                    }
                    let loc = path.parent().map(|d| d.to_string_lossy().replace('\\', "\\\u{200B}")).unwrap_or_default();
                    card1.push(row(p, t!("Konum"), loc));
                    if i.is_dir {
                        let (size, contents) = match &self.folder {
                            Some(Some(fs)) => (
                                format!("{} ({} {})", format::size(fs.bytes), group(fs.bytes), t!("bayt")),
                                format!("{} {}, {} {}", group(fs.files), t!("dosya"), group(fs.dirs), t!("klasör")),
                            ),
                            Some(None) if self.folder.is_some() => (t!("Hesaplanıyor…").into(), t!("Hesaplanıyor…").into()),
                            _ => ("—".into(), "—".into()),
                        };
                        card1.push(row(p, t!("Boyut"), size));
                        card1.push(row(p, t!("İçerik"), contents));
                    } else {
                        card1.push(row(p, t!("Boyut"), format!("{} ({} {})", format::size(i.size), group(i.size), t!("bayt"))));
                        if let Some(d) = i.size_on_disk {
                            card1.push(row(p, t!("Disk üzerindeki boyut"), format!("{} ({} {})", format::size(d), group(d), t!("bayt"))));
                        }
                    }
                }
            }
        } else if let Some(t) = &self.totals {
            let dir = self.paths[0].parent().map(|d| d.to_string_lossy().replace('\\', "\\\u{200B}")).unwrap_or_default();
            card1.push(row(p, t!("Seçim"), crate::i18n::items(self.paths.len())));
            card1.push(row(p, t!("Konum"), dir));
            let suffix = if t.done { String::new() } else { format!(" — {}", t!("hesaplanıyor…")) };
            card1.push(row(p, t!("Boyut"), format!("{} ({} {}){suffix}", format::size(t.bytes), group(t.bytes), t!("bayt"))));
            card1.push(row(p, t!("İçerik"), format!("{} {}, {} {}", group(t.files), t!("dosya"), group(t.dirs), t!("klasör"))));
        }
        out.push(card(p, card1));

        // Files' "Shortcut" tab: where it points
        if let Some((target, args, dir)) = &self.shortcut {
            let mut rows = vec![row(p, t!("Hedef"), target.replace('\\', "\\\u{200B}"))];
            if !args.is_empty() {
                rows.push(row(p, t!("Bağımsız değişkenler"), args.clone()));
            }
            if !dir.is_empty() {
                rows.push(row(p, t!("Başlangıç yeri"), dir.replace('\\', "\\\u{200B}")));
            }
            out.push(card(p, rows));
            let t = std::path::PathBuf::from(target);
            out.push(
                h_flex()
                    .id("open-target")
                    .gap_1p5()
                    .items_center()
                    .text_sm()
                    .text_color(p.accent)
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.8))
                    .child(Icon::new(L::FolderOpen).size(px(14.)))
                    .child(t!("Hedefin konumunu aç"))
                    // opens the folder as a tab in the app window (the second-launch route)
                    .on_click(move |_, _, cx| {
                        let dir = if t.extension().is_some() { t.parent().map(Path::to_path_buf).unwrap_or_else(|| t.clone()) } else { t.clone() };
                        crate::workspace::route_ipc(dir.to_string_lossy().into_owned(), cx);
                    })
                    .into_any_element(),
            );
        }

        if let Some(Ok(i)) = &self.info {
            out.push(card(
                p,
                vec![
                    row(p, t!("Oluşturulma"), format::datetime_full(i.created_ms)),
                    row(p, t!("Değiştirilme"), format::datetime_full(i.modified_ms)),
                    row(p, t!("Erişim"), format::datetime_full(i.accessed_ms)),
                ],
            ));
        }

        // attributes
        let ro = self.readonly;
        let hid = self.hidden;
        out.push(card(
            p,
            vec![
                check_row(p, "ro", t!("Salt okunur"), ro, cx.listener(move |v, _, _, cx| {
                    v.readonly = Some(!ro.unwrap_or(false));
                    cx.notify();
                })),
                check_row(p, "hid", t!("Gizli"), hid, cx.listener(move |v, _, _, cx| {
                    v.hidden = Some(!hid.unwrap_or(false));
                    cx.notify();
                })),
            ],
        ));
        out.push(
            h_flex()
                .id("win-props")
                .gap_1p5()
                .items_center()
                .text_sm()
                .text_color(p.accent)
                .cursor_pointer()
                .hover(|s| s.opacity(0.8))
                .child(Icon::new(L::ExternalLink).size(px(14.)))
                .child(t!("Windows özellikleri (güvenlik, paylaşım, önceki sürümler)…"))
                .on_click(cx.listener(|v, _, _, _| v.windows_properties()))
                .into_any_element(),
        );
        out
    }

    fn render_details(&self, p: &Palette) -> Vec<AnyElement> {
        if self.media.is_empty() {
            return vec![
                div()
                    .text_sm()
                    .text_color(p.text_secondary)
                    .child(t!("Bu dosya için ek ayrıntı yok"))
                    .into_any_element(),
            ];
        }
        let rows = self.media.iter().map(|(k, v)| row(p, crate::i18n::t(k), v.clone())).collect();
        vec![card(p, rows)]
    }
}

fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push('.');
        }
        out.push(c);
    }
    out
}

fn row(p: &Palette, k: &'static str, v: String) -> AnyElement {
    h_flex()
        .w_full()
        .gap_3()
        .items_start()
        .py(px(7.))
        .text_sm()
        .child(div().w(px(150.)).flex_shrink_0().text_color(p.text_secondary).child(k))
        .child(div().flex_1().min_w_0().child(v))
        .into_any_element()
}

fn card(p: &Palette, rows: Vec<AnyElement>) -> AnyElement {
    v_flex()
        .w_full()
        .px_4()
        .py_2()
        .rounded(theme::RADIUS)
        .bg(p.card_bg)
        .border_1()
        .border_color(p.border)
        .children(rows)
        .into_any_element()
}

fn check_row(
    p: &Palette,
    id: &'static str,
    label: &'static str,
    on: Option<bool>,
    click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let (fill, mark) = match on {
        Some(true) => (Some(p.accent), Some(L::Check)),
        None => (Some(p.accent), Some(L::Minus)),
        Some(false) => (None, None),
    };
    h_flex()
        .id(id)
        .gap_3()
        .py(px(7.))
        .items_center()
        .text_sm()
        .cursor_pointer()
        .child(
            div()
                .size(px(18.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .when_some(fill, |d, c| d.bg(c))
                .when(fill.is_none(), |d| d.border_1().border_color(p.text_secondary))
                .children(mark.map(|m| Icon::new(m).size(px(12.)).text_color(p.accent_text))),
        )
        .child(label)
        .on_click(click)
        .into_any_element()
}

impl Render for PropsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::for_window(window);
        if self.frame_dark != Some(p.dark) {
            self.frame_dark = Some(p.dark);
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            if let Ok(h) = HasWindowHandle::window_handle(window) {
                if let RawWindowHandle::Win32(w) = h.as_raw() {
                    rf_win::window::set_dark_mode(w.hwnd.get(), p.dark);
                }
            }
        }
        let title = match self.single() {
            Some(path) => format!("{} — {}", file_name(path), t!("Özellikler")),
            None => format!("{} — {}", crate::i18n::items(self.paths.len()), t!("Özellikler")),
        };
        window.set_window_title(&title);

        let is_file = matches!(&self.info, Some(Ok(i)) if !i.is_dir);
        // header: big icon / thumbnail + name
        let icon: AnyElement = match self.entry() {
            Some(e) => {
                let dir = self.paths[0].parent().map(Path::to_path_buf).unwrap_or_default();
                match self.images.update(cx, |c, cx| c.thumbnail(&dir, &e, 96, cx)) {
                    Some(im) => img(im).size(px(48.)).object_fit(ObjectFit::Contain).into_any_element(),
                    None => {
                        let cat = crate::icons::category(&e);
                        Icon::new(crate::icons::icon_for(cat)).size(px(40.)).text_color(theme::kind_color(cat, p.dark)).into_any_element()
                    }
                }
            }
            None => Icon::new(if self.paths.len() > 1 { L::Files } else { L::File }).size(px(40.)).text_color(p.text_secondary).into_any_element(),
        };
        let name: AnyElement = match &self.name {
            Some(n) => div()
                .flex_1()
                .h(px(34.))
                .px_2()
                .flex()
                .items_center()
                .rounded(theme::RADIUS_SM)
                .bg(p.content_bg)
                .border_1()
                .border_color(p.border)
                .child(Input::new(n).appearance(false))
                .into_any_element(),
            None => div()
                .flex_1()
                .text_base()
                .font_weight(FontWeight::SEMIBOLD)
                .child(crate::i18n::items(self.paths.len()))
                .into_any_element(),
        };

        // tabs
        let mut tabs = h_flex().p(px(2.)).gap(px(2.)).rounded(theme::RADIUS_SM).bg(p.content_bg).border_1().border_color(p.border);
        let mut pages = vec![(Page::General, t!("Genel"))];
        if is_file {
            pages.push((Page::Details, t!("Ayrıntılar")));
            pages.push((Page::Hashes, t!("Karmalar")));
        }
        if !pages.iter().any(|(pg, _)| *pg == self.page) {
            self.page = Page::General;
        }
        for (pg, label) in pages {
            let on = pg == self.page;
            tabs = tabs.child(
                div()
                    .id(label)
                    .flex_1()
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.))
                    .text_sm()
                    .when(on, |d| d.bg(p.card_bg).shadow_sm().font_weight(FontWeight::MEDIUM))
                    .when(!on, |d| d.text_color(p.text_secondary).hover(|s| s.bg(p.hover)))
                    .child(label)
                    .on_click(cx.listener(move |v, _, _, cx| {
                        v.page = pg;
                        if pg == Page::Hashes && v.hash.is_none() {
                            let path = v.paths[0].clone();
                            v.hash = Some(cx.new(|cx| crate::hash_panel::HashPanel::new(path, cx)));
                        }
                        cx.notify();
                    })),
            );
        }

        let body: Vec<AnyElement> = match self.page {
            Page::General => self.render_general(&p, cx),
            Page::Details => self.render_details(&p),
            Page::Hashes => self.hash.clone().map(|h| vec![card(&p, vec![div().py_2().child(h).into_any_element()])]).unwrap_or_default(),
        };

        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .min_w(px(88.))
                .h(px(32.))
                .px_4()
                .flex()
                .items_center()
                .justify_center()
                .rounded(theme::RADIUS_SM)
                .text_sm()
                .when(primary, |d| d.bg(p.accent).text_color(p.accent_text))
                .when(!primary, |d| d.bg(p.hover).border_1().border_color(p.border))
                .hover(|s| s.opacity(0.9))
                .active(|s| s.opacity(0.8))
                .child(label)
        };
        let dirty = self.dirty(cx);

        v_flex()
            .size_full()
            .bg(p.window_bg)
            .text_color(p.text)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|v, ev: &KeyDownEvent, window, cx| match ev.keystroke.key.as_str() {
                "escape" => window.remove_window(),
                "enter" => v.apply(true, window, cx),
                _ => {}
            }))
            .child(
                TitleBar::new()
                    .h(theme::TITLEBAR_H)
                    .bg(p.titlebar_bg)
                    .border_color(gpui_kit::transparent_black())
                    .child(h_flex().gap_2().items_center().text_xs().child(Icon::new(L::Info).size(px(14.))).child(title.clone())),
            )
            .child(
                v_flex()
                    .id("props-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_5()
                    .pt_2()
                    .pb_4()
                    .gap_4()
                    .child(h_flex().gap_3().items_center().child(div().size(px(48.)).flex().items_center().justify_center().child(icon)).child(name))
                    .when(is_file, |d| d.child(tabs))
                    .children(body),
            )
            .child(
                h_flex()
                    .px_5()
                    .py_3()
                    .gap_2()
                    .items_center()
                    .border_t_1()
                    .border_color(p.divider)
                    .child(div().flex_1().min_w_0().text_xs().text_color(p.danger).children(self.error.clone()))
                    .child(button("ok", t!("Tamam"), true).on_click(cx.listener(|v, _, window, cx| v.apply(true, window, cx))))
                    .child(button("cancel", t!("İptal"), false).on_click(|_, window, _| window.remove_window()))
                    .child(
                        button("apply", t!("Uygula"), false)
                            .when(!dirty, |d| d.opacity(0.5))
                            .on_click(cx.listener(|v, _, window, cx| v.apply(false, window, cx))),
                    ),
            )
    }
}
