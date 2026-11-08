//! rustyfiles as the Open / Save dialog of other apps (see crates/rf-dialog). The DLL in the
//! app starts `rustyfiles --pick <pipe>`; we read its request from the pipe, open a normal
//! rustyfiles window with a strip at the bottom (name, file type, OK / Cancel) and write the
//! answer back. Opening a file in that window (double click, Enter) picks it.

use std::cell::{Cell, RefCell};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui_kit::component::{Icon, h_flex};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rf_core::Location;
use rustyfiles_dialog::proto::{Mode, Request, Response, first_ext, read_msg, write_msg};

use crate::t;
use crate::theme::{self, Palette};
use crate::workspace::Workspace;

/// Single-instance message: "pick this pipe's request".
pub const PICK_PREFIX: &str = "\u{1}pick:";

pub struct PickSession {
    pub req: Request,
    /// Taken by the answer: each request is answered exactly once.
    pipe: RefCell<Option<File>>,
    pub name: Entity<InputState>,
    /// 1-based, as in the request.
    pub filter_ix: Cell<u32>,
    /// The selection the name box was last filled from.
    last_selection: RefCell<Vec<PathBuf>>,
    _sub: RefCell<Option<Subscription>>,
}

impl PickSession {
    /// The patterns of the chosen file type ("" = all files).
    pub fn patterns(&self) -> String {
        self.req.filters.get(self.filter_ix.get().saturating_sub(1) as usize).map(|f| f.spec.clone()).unwrap_or_default()
    }

    pub fn folders_only(&self) -> bool {
        self.req.mode == Mode::Folder
    }

    /// Written right here, before the window closes: when rustyfiles was started only for this
    /// dialog it quits with its last window, and an answer left to another thread could be lost.
    /// The app is already waiting to read, so the write doesn't block.
    fn answer(&self, resp: Response) {
        if let Some(mut f) = self.pipe.borrow_mut().take() {
            if let Err(e) = write_msg(&mut f, &resp) {
                log::warn!("picker answer: {e}");
            }
        }
    }
}

impl Drop for PickSession {
    /// Whatever closed the window, the app must not wait forever: an unanswered request is
    /// cancelled.
    fn drop(&mut self) {
        self.answer(Response { ok: false, paths: Vec::new(), filter_index: self.filter_ix.get() });
    }
}

/// `--pick <pipe>` (or the same message from a second launch): read the request, open the window.
pub fn begin(pipe: String, cx: &mut App) {
    let task = cx.background_spawn(async move {
        let mut f = std::fs::OpenOptions::new().read(true).write(true).open(&pipe)?;
        let req: Request = read_msg(&mut f)?;
        Ok::<_, std::io::Error>((f, req))
    });
    cx.spawn(async move |cx| match task.await {
        Ok((f, req)) => {
            cx.update(|cx| open_window(f, req, cx));
        }
        Err(e) => log::warn!("picker request: {e}"),
    })
    .detach();
}

fn mode_title(mode: Mode) -> &'static str {
    match mode {
        Mode::Open => t!("Aç"),
        Mode::Save => t!("Farklı kaydet"),
        Mode::Folder => t!("Klasör seç"),
    }
}

fn ok_label(req: &Request) -> SharedString {
    if !req.ok_label.trim().is_empty() {
        return req.ok_label.replace('&', "").into();
    }
    match req.mode {
        Mode::Open => t!("Aç"),
        Mode::Save => t!("Kaydet"),
        Mode::Folder => t!("Klasörü seç"),
    }
    .into()
}

fn open_window(pipe: File, req: Request, cx: &mut App) {
    let start = req.folder.as_deref().map(PathBuf::from).filter(|p| p.is_dir()).map(Location::Path).unwrap_or(Location::Home);
    let title = {
        let base = if req.title.trim().is_empty() { mode_title(req.mode).to_string() } else { req.title.clone() };
        if req.app.is_empty() { base } else { format!("{base} — {}", req.app.trim_end_matches(".exe")) }
    };
    let owner = req.owner;
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(size(px(1040.), px(660.)), cx)),
        window_min_size: Some(size(px(640.), px(400.))),
        window_background: WindowBackgroundAppearance::MicaBackdrop,
        ..gpui_kit::component::TitleBar::window_options()
    };
    let pipe = RefCell::new(Some(pipe));
    let opened = gpui_kit::open_window(options, cx, move |window, cx| {
        window.set_window_title(&title);
        let hwnd = crate::look::window::hwnd_of(window);
        if owner != 0 {
            // stays above the app that asked, like its own dialog would
            rf_win::window::set_owner(hwnd, owner);
        }
        rf_win::window::bring_to_front(hwnd);
        let pipe = pipe.borrow_mut().take();
        cx.new(|cx| {
            let mut ws = Workspace::new(Some(start), None, window, cx);
            let name = cx.new(|cx| InputState::new(window, cx).default_value(req.file_name.clone()));
            let session = Rc::new(PickSession {
                filter_ix: Cell::new(req.filter_index.max(1)),
                req,
                pipe: RefCell::new(pipe),
                name: name.clone(),
                last_selection: RefCell::new(Vec::new()),
                _sub: RefCell::new(None),
            });
            let sub = cx.subscribe_in(&name, window, |ws: &mut Workspace, _, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    accept(ws, window, cx);
                }
            });
            *session._sub.borrow_mut() = Some(sub);
            ws.set_picker(session, window, cx);
            ws
        })
    });
    if let Err(e) = opened {
        log::error!("picker window: {e:#}");
    }
}

/// The selected entries of the current folder, with whether each is a folder.
fn selection(ws: &Workspace, cx: &App) -> Vec<(PathBuf, bool)> {
    ws.active_view().read(cx).selection_with_kind()
}

fn names_text(paths: &[PathBuf]) -> String {
    let name = |p: &PathBuf| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    match paths {
        [one] => name(one),
        _ => paths.iter().map(|p| format!("\"{}\"", name(p))).collect::<Vec<_>>().join(" "),
    }
}

/// `"a.txt" "b.txt"` or `a.txt` → names.
fn parse_names(text: &str) -> Vec<String> {
    let t = text.trim();
    if !t.contains('"') {
        return if t.is_empty() { Vec::new() } else { vec![t.to_string()] };
    }
    t.split('"').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect()
}

fn resolve(dir: Option<&Path>, name: &str) -> Option<PathBuf> {
    let p = Path::new(name);
    if p.is_absolute() {
        return Some(p.to_path_buf());
    }
    dir.map(|d| d.join(name))
}

/// Keeps the name box in step with the selection (files when opening or saving, a folder when
/// picking one).
fn sync_name(ws: &Workspace, s: &PickSession, window: &mut Window, cx: &mut App) {
    let sel = selection(ws, cx);
    let wanted: Vec<PathBuf> = sel.iter().filter(|(_, dir)| *dir == (s.req.mode == Mode::Folder)).map(|(p, _)| p.clone()).collect();
    if wanted.is_empty() || *s.last_selection.borrow() == wanted {
        return;
    }
    *s.last_selection.borrow_mut() = wanted.clone();
    let text = names_text(&wanted);
    s.name.update(cx, |st, cx| st.set_value(text, window, cx));
}

fn finish(ws: &mut Workspace, paths: Vec<PathBuf>, window: &mut Window) {
    if let Some(s) = ws.picker() {
        s.answer(Response { ok: true, paths: paths.iter().map(|p| p.to_string_lossy().into_owned()).collect(), filter_index: s.filter_ix.get() });
    }
    window.remove_window();
}

pub(crate) fn cancel(ws: &mut Workspace, window: &mut Window) {
    answer_cancel(ws);
    window.remove_window();
}

/// The window is closing anyway (X, Alt+F4): just tell the app.
pub(crate) fn answer_cancel(ws: &mut Workspace) {
    if let Some(s) = ws.picker() {
        s.answer(Response { ok: false, paths: Vec::new(), filter_index: s.filter_ix.get() });
    }
}

/// Files opened in the window (double click, Enter on the list).
pub(crate) fn picked(ws: &mut Workspace, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Workspace>) {
    let Some(s) = ws.picker() else { return };
    match s.req.mode {
        Mode::Folder => {}
        Mode::Open => {
            let paths = if s.req.multi { paths } else { paths.into_iter().take(1).collect() };
            finish(ws, paths, window);
        }
        // saving over an existing file: the same question as the button
        Mode::Save => {
            if let Some(p) = paths.into_iter().next() {
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                s.name.update(cx, |st, cx| st.set_value(name, window, cx));
                accept(ws, window, cx);
            }
        }
    }
}

/// OK button / Enter in the name box.
pub(crate) fn accept(ws: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let Some(s) = ws.picker() else { return };
    let view = ws.active_view();
    let dir = view.read(cx).current_dir();
    let text = s.name.read(cx).value().trim().to_string();
    let sel = selection(ws, cx);
    let navigate = |ws: &mut Workspace, to: PathBuf, window: &mut Window, cx: &mut Context<Workspace>| {
        ws.active_view().update(cx, |v, cx| v.navigate(Location::Path(to), cx));
        s.name.update(cx, |st, cx| st.set_value("", window, cx));
    };
    match s.req.mode {
        Mode::Folder => {
            let chosen = match sel.as_slice() {
                [(p, true)] => Some(p.clone()),
                _ => resolve(dir.as_deref(), &text).filter(|p| !text.is_empty() && p.is_dir()).or(dir),
            };
            match chosen {
                Some(p) => finish(ws, vec![p], window),
                None => tell(t!("Bir klasöre girin ya da bir klasör seçin."), window, cx),
            }
        }
        Mode::Open => {
            let files: Vec<PathBuf> = sel.iter().filter(|(_, d)| !d).map(|(p, _)| p.clone()).collect();
            if !files.is_empty() && text == names_text(&files) {
                return finish(ws, if s.req.multi { files } else { files.into_iter().take(1).collect() }, window);
            }
            let names = parse_names(&text);
            if names.is_empty() {
                if let [(p, true)] = sel.as_slice() {
                    navigate(ws, p.clone(), window, cx);
                }
                return;
            }
            let targets: Vec<PathBuf> = names.iter().filter_map(|n| resolve(dir.as_deref(), n)).collect();
            if let [one] = targets.as_slice() {
                if one.is_dir() {
                    return navigate(ws, one.clone(), window, cx);
                }
            }
            if targets.is_empty() || targets.iter().any(|t| !t.is_file()) {
                return tell(t!("Dosya bulunamadı. Adı ve klasörü denetleyin."), window, cx);
            }
            finish(ws, if s.req.multi { targets } else { targets.into_iter().take(1).collect() }, window);
        }
        Mode::Save => {
            if text.is_empty() {
                return;
            }
            let Some(mut target) = resolve(dir.as_deref(), &text) else {
                return tell(t!("Önce kaydedilecek klasörü açın."), window, cx);
            };
            if target.is_dir() {
                return navigate(ws, target, window, cx);
            }
            if target.extension().is_none() {
                let ext = if s.req.default_ext.is_empty() { first_ext(&s.patterns()) } else { Some(s.req.default_ext.clone()) };
                if let Some(ext) = ext {
                    target.set_extension(ext);
                }
            }
            if target.exists() && s.req.overwrite_prompt {
                let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let ws_handle = cx.entity().downgrade();
                let t2 = target.clone();
                crate::prompt::show(
                    format!("{name} {}", t!("zaten var.")),
                    Some(t!("Değiştirmek istiyor musunuz?").to_string()),
                    None,
                    vec![
                        crate::prompt::Button::new(t!("Değiştir"), crate::prompt::Kind::Primary, move |_, window, cx| {
                            if let Some(ws) = ws_handle.upgrade() {
                                let p = t2.clone();
                                ws.update(cx, |ws, _| finish(ws, vec![p], window));
                            }
                            Ok(())
                        }),
                        crate::prompt::Button::cancel(),
                    ],
                    window,
                    cx,
                );
                return;
            }
            finish(ws, vec![target], window);
        }
    }
}

fn tell(msg: &str, window: &mut Window, cx: &mut App) {
    crate::prompt::show(msg.to_string(), None, None, vec![crate::prompt::Button::new(t!("Tamam"), crate::prompt::Kind::Primary, |_, _, _| Ok(()))], window, cx);
}

/// The strip at the bottom of the window.
pub(crate) fn footer(ws: &mut Workspace, s: &Rc<PickSession>, p: &Palette, window: &mut Window, cx: &mut Context<Workspace>) -> AnyElement {
    sync_name(ws, s, window, cx);
    let label = match s.req.mode {
        Mode::Folder => t!("Klasör:"),
        _ => t!("Dosya adı:"),
    };
    let filter = (!s.req.filters.is_empty() && s.req.mode != Mode::Folder).then(|| {
        let current: SharedString =
            s.req.filters.get(s.filter_ix.get().saturating_sub(1) as usize).map(|f| f.name.clone()).unwrap_or_default().into();
        let me = cx.entity().downgrade();
        let filters = s.req.filters.clone();
        h_flex()
            .id("pick-filter")
            .h(px(32.))
            .px_3()
            .gap_2()
            .max_w(px(320.))
            .rounded(theme::RADIUS_SM)
            .border_1()
            .border_color(p.border)
            .bg(p.card_bg)
            .hover(|d| d.bg(p.hover))
            .cursor_pointer()
            .text_sm()
            .child(div().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(current))
            .child(Icon::new(gpui_kit::assets::IconName::ChevronDown).size(px(14.)))
            .on_click(move |ev, window, cx| {
                use crate::fluent_menu::{Item, Menu, MenuIcon};
                let items = filters
                    .iter()
                    .enumerate()
                    .map(|(i, f)| {
                        let me = me.clone();
                        Item::action(f.name.clone(), MenuIcon::None, move |window, cx| {
                            if let Some(ws) = me.upgrade() {
                                ws.update(cx, |ws, cx| ws.set_pick_filter(i as u32 + 1, window, cx));
                            }
                        })
                    })
                    .collect();
                crate::fluent_menu::show(Menu { quick: Vec::new(), items }, ev.position(), window, cx);
            })
    });
    let button = |id: &'static str, label: SharedString, primary: bool| {
        div()
            .id(id)
            .h(px(32.))
            .min_w(px(96.))
            .px_4()
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme::RADIUS_SM)
            .text_sm()
            .cursor_pointer()
            .when(primary, |d| d.bg(p.accent).text_color(gpui_kit::rgb(0xFFFFFF)).hover(|d| d.opacity(0.9)))
            .when(!primary, |d| d.border_1().border_color(p.border).bg(p.card_bg).hover(|d| d.bg(p.hover)))
            .child(label)
    };
    // two rows, like Windows' own dialog: the name gets the whole width (long paths fit)
    gpui_kit::component::v_flex()
        .w_full()
        .px_4()
        .py_3()
        .gap_2()
        .border_t_1()
        .border_color(p.divider)
        .child(
            h_flex()
                .w_full()
                .gap_3()
                .items_center()
                .child(div().w(px(80.)).text_sm().text_color(p.text_secondary).flex_shrink_0().child(label))
                .child(div().flex_1().min_w_0().child(Input::new(&s.name)))
                .children(filter),
        )
        .child(
            h_flex()
                .w_full()
                .gap_3()
                .justify_end()
                .child(button("pick-ok", ok_label(&s.req), true).on_click(cx.listener(|ws, _, window, cx| accept(ws, window, cx))))
                .child(button("pick-cancel", t!("İptal").into(), false).on_click(cx.listener(|ws, _, window, _| cancel(ws, window)))),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    // not `super::*`: gpui's `test` macro would shadow the built-in one
    use super::{names_text, parse_names, resolve};
    use std::path::{Path, PathBuf};

    #[test]
    fn names_round_trip() {
        let ps = vec![PathBuf::from(r"C:\a\bir.txt"), PathBuf::from(r"C:\a\iki dosya.txt")];
        assert_eq!(names_text(&ps), "\"bir.txt\" \"iki dosya.txt\"");
        assert_eq!(parse_names(&names_text(&ps)), vec!["bir.txt", "iki dosya.txt"]);
        assert_eq!(parse_names("  rapor.pdf "), vec!["rapor.pdf"]);
        assert!(parse_names("").is_empty());
        assert_eq!(resolve(Some(Path::new(r"C:\a")), "b.txt"), Some(PathBuf::from(r"C:\a\b.txt")));
        assert_eq!(resolve(None, r"D:\x.txt"), Some(PathBuf::from(r"D:\x.txt")));
        assert_eq!(resolve(None, "x.txt"), None);
    }
}
