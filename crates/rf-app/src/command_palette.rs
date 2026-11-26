//! Command palette (Ctrl+Shift+P / F1): fuzzy-filter every command and the folders visited before (`folder_history`, unless
//! `palette_folder_history` is off), or type a path to jump. Opened by Ctrl+P (`SearchFolderHistory`) it lists past folders only;
//! opened by Ctrl+Shift+A (`SearchTabs`) the window's open tabs and the ones closed recently (issue 79, as FilePilot's tab search).

use std::path::PathBuf;

use gpui_kit::assets::IconName as L;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::actions::*;
use crate::theme::{self, Palette};

struct Cmd {
    label: &'static str,
    keys: &'static str,
    icon: L,
    action: fn() -> Box<dyn Action>,
}

macro_rules! cmd {
    ($label:expr, $keys:expr, $icon:expr, $act:expr) => {
        Cmd { label: crate::i18n::t($label), keys: $keys, icon: $icon, action: || Box::new($act) }
    };
}

fn commands() -> Vec<Cmd> {
    vec![
        cmd!("Yeni sekme", "Ctrl+T", L::Plus, NewTab),
        cmd!("Yeni pencere", "Ctrl+N", L::AppWindow, NewWindow),
        cmd!("Sekmeyi kapat", "Ctrl+W", L::X, CloseTab),
        cmd!("Sekmeyi çoğalt", "Ctrl+Shift+K", L::Copy, DuplicateTab),
        cmd!("Diğer sekmeleri kapat", "", L::X, CloseOtherTabs),
        cmd!("Tüm sekmeleri kapat", "", L::X, CloseAllTabs),
        cmd!("Kapatılan sekmeyi yeniden aç", "Ctrl+Shift+T", L::RotateCcw, ReopenClosedTab),
        cmd!("Yeni klasör", "Ctrl+Shift+N", L::FolderPlus, NewFolder),
        cmd!("Yeni metin belgesi", "", L::FilePlus, NewTextFile),
        cmd!("Bağlantıdan indir…", "", L::Download, DownloadFromLink),
        cmd!("Yenile", "F5", L::RotateCw, Refresh),
        cmd!("Ağ sürücüsüne bağlan…", "", L::Network, MapNetworkDrive),
        cmd!("rustyfiles'ı yeniden başlat", "Ctrl+Alt+Shift+R", L::RotateCw, RestartApp),
        cmd!("Oturumu kaydet…", "", L::Save, SaveSession),
        cmd!("Ağ sürücüsü bağlantısını kes…", "", L::Network, DisconnectNetworkDrive),
        cmd!("Geri", "Alt+←", L::ArrowLeft, GoBack),
        cmd!("İleri", "Alt+→", L::ArrowRight, GoForward),
        cmd!("Üst klasör", "Alt+↑", L::ArrowUp, GoUp),
        cmd!("Ana sayfa", "Alt+Home", L::House, GoHome),
        cmd!("Ayarlar", "Ctrl+,", L::Settings, GoSettings),
        cmd!("Geri dönüşüm kutusu", "", L::Trash, GoRecycleBin),
        cmd!("Adres çubuğuna git", "Ctrl+L", L::TextCursorInput, FocusAddress),
        cmd!("Ara", "Ctrl+F", L::Search, FocusSearch),
        cmd!("Gizli öğeleri göster/gizle", "Ctrl+H", L::Eye, ToggleHidden),
        cmd!("Detay görünümü", "Ctrl+Shift+1", L::List, ViewDetails),
        cmd!("Izgara görünümü", "Ctrl+Shift+2", L::LayoutGrid, ViewGrid),
        cmd!("Sütun görünümü", "Ctrl+Shift+3", L::Columns3, ViewColumns),
        cmd!("Kart görünümü", "Ctrl+Shift+4", L::LayoutList, ViewCards),
        cmd!("Liste görünümü", "Ctrl+Shift+5", L::TextAlignJustify, ViewList),
        cmd!("Simgeleri büyüt", "Ctrl+=", L::ZoomIn, ZoomIn),
        cmd!("Rafa ekle", "Ctrl+Shift+S", L::Inbox, AddToShelf),
        cmd!("Rafı göster/gizle", "", L::Inbox, ToggleShelf),
        cmd!("Tam ekran", "F11", L::Maximize, ToggleFullscreen),
        cmd!("Her zaman üstte", "", L::Pin, ToggleAlwaysOnTop),
        cmd!("Kompakt görünüm", "", L::PictureInPicture2, ToggleCompactOverlay),
        cmd!("Klasör takma adına git", "@", L::AtSign, GoToFolderAlias),
        cmd!("Geçmiş klasörlerde ara", "Ctrl+P", L::Clock, SearchFolderHistory),
        cmd!("Sekmelerde ara", "Ctrl+Shift+A", L::PanelTop, SearchTabs),
        cmd!("Sağa döndür", "", L::RotateCw, RotateRight),
        cmd!("Sola döndür", "", L::RotateCcw, RotateLeft),
        cmd!("Simgeleri küçült", "Ctrl+-", L::ZoomOut, ZoomOut),
        cmd!("Çift panel aç/kapat", "Ctrl+Shift+D", L::Columns2, ToggleDualPane),
        cmd!("Panel değiştir", "F6", L::ArrowLeftRight, SwitchPane),
        cmd!("Bölmeyi yan yana böl", "", L::Columns2, SplitPaneSideBySide),
        cmd!("Bölmeyi üst üste böl", "", L::Rows2, SplitPaneStacked),
        cmd!("Etkin bölmeyi kapat", "Ctrl+Shift+W", L::X, CloseActivePane),
        cmd!("Önizleme paneli", "Alt+P", L::PanelRight, TogglePreview),
        cmd!("Kenar çubuğu", "Ctrl+B", L::PanelLeft, ToggleSidebar),
        cmd!("Terminalde aç", "Ctrl+Shift+`", L::SquareTerminal, OpenTerminal),
        cmd!("Terminalde yönetici olarak aç", "", L::ShieldCheck, OpenTerminalAdmin),
        cmd!("Düzenleyicide aç", "", L::Code, OpenInIde),
        cmd!("Metin düzenleyicide düzenle", "F4", L::FilePen, EditFiles),
        cmd!("Klasörü düzenle", "", L::WandSparkles, OrganizeFolder),
        cmd!("Açık / koyu tema", "Ctrl+Alt+T", L::SunMoon, ToggleTheme),
        cmd!("Yeni pencerede aç", "Ctrl+Alt+Enter", L::AppWindow, OpenInNewWindow),
        cmd!("Tümünü seç", "Ctrl+A", L::SquareCheck, SelectAll),
        cmd!("Seçimi tersine çevir", "", L::ArrowLeftRight, InvertSelection),
        cmd!("Tüm grupları daralt / genişlet", "Ctrl+Shift+G", L::ChevronsDownUp, ToggleAllGroups),
        cmd!("Öğeyi seç/bırak", "Ctrl+Space", L::SquareCheck, ToggleSelection),
        cmd!("Kopyala", "Ctrl+C", L::Copy, CopyItems),
        cmd!("Kes", "Ctrl+X", L::Scissors, CutItems),
        cmd!("Yapıştır", "Ctrl+V", L::ClipboardPaste, PasteItems),
        cmd!("Seçili klasöre yapıştır", "Ctrl+Shift+V", L::ClipboardPaste, PasteIntoSelected),
        cmd!("Kısayol olarak yapıştır", "", L::Link, PasteShortcut),
        cmd!("Geri dönüşüm kutusuna taşı", "Del", L::Trash, DeleteItems),
        cmd!("Kalıcı olarak sil", "Shift+Del", L::TrashOff, DeletePermanently),
        cmd!("Yeniden adlandır", "F2", L::Pencil, RenameItem),
        cmd!("Çoğalt", "Ctrl+Alt+D", L::CopyPlus, DuplicateItems),
        cmd!("Diğer panele kopyala", "Shift+F5", L::CopyPlus, CopyToOtherPane),
        cmd!("Diğer panele taşı", "Shift+F6", L::MoveRight, MoveToOtherPane),
        cmd!("Geri al", "Ctrl+Z", L::Undo2, Undo),
        cmd!("Yolu kopyala", "Ctrl+Shift+C", L::Link, CopyPath),
        cmd!("Birlikte aç…", "", L::AppWindow, OpenWith),
        cmd!("Özellikler", "Alt+Enter", L::Info, Properties),
        cmd!("Sağ tık menüsü", "Shift+F10", L::Menu, ShowContextMenu),
    ]
}

/// (label, shortcut) for every command that has one (shown in Settings).
/// Every command for the shortcut editor: (label, default keys, action short name).
pub fn shortcut_entries() -> Vec<(&'static str, &'static str, String)> {
    commands()
        .into_iter()
        .map(|c| {
            let name = (c.action)().name().rsplit("::").next().unwrap_or_default().to_string();
            (c.label, c.keys, name)
        })
        .collect()
}

#[allow(dead_code)]
pub fn shortcut_list() -> Vec<(&'static str, &'static str)> {
    commands().into_iter().filter(|c| !c.keys.is_empty()).map(|c| (c.label, c.keys)).collect()
}

pub enum PaletteEvent {
    Dismiss,
    Run(Box<dyn Action>),
    Navigate(PathBuf),
    /// A tab of the tab search: an open one to switch to, or a closed one to open again (`TabEntry::id`).
    Tab { closed: bool, id: usize },
}

/// One tab the tab search lists.
#[derive(Clone, Debug, PartialEq)]
pub struct TabEntry {
    pub label: String,
    /// The tab's place, written out (its path), matched as well.
    pub detail: String,
    pub closed: bool,
    /// The open tab's index in the window, or the closed one's in the window's list of closed tabs.
    pub id: usize,
}

impl EventEmitter<PaletteEvent> for CommandPalette {}

/// What the palette lists.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PaletteMode {
    /// Commands, and past folders once something is typed (when the setting allows).
    Commands { with_folders: bool },
    /// Past folders only (Ctrl+P).
    Folders,
    /// The window's tabs, open and recently closed (Ctrl+Shift+A).
    Tabs,
}

/// One row under the typed path's row.
#[derive(Clone, Debug, PartialEq)]
enum Row {
    /// An index into `cmds`.
    Cmd(usize),
    Folder(PathBuf),
    /// An index into `tabs`.
    Tab(usize),
}

/// How many rows are listed under the typed path's row. The tab search lists every tab and shows this many at a time, the rest
/// by scrolling (issue 79: "it can only display a maximum of 12 tabs ... add a scrollbar").
const VISIBLE: usize = 12;
/// A row's height and the gap between two rows.
const ROW_H: f32 = 34.;
const ROW_GAP: f32 = 2.;
/// At most this many past folders among the commands (the folder-only mode lists [`VISIBLE`]).
const FOLDERS_AMONG_COMMANDS: usize = 5;

pub struct CommandPalette {
    input: Entity<InputState>,
    mode: PaletteMode,
    cmds: Vec<Cmd>,
    folders: Vec<crate::folder_history::Visit>,
    tabs: Vec<TabEntry>,
    /// What matches the query, best first (plus an optional path row at the top).
    filtered: Vec<Row>,
    path: Option<PathBuf>,
    selected: usize,
    /// The list's scroll place (the tab search's list can be longer than what shows).
    scroll: ScrollHandle,
    _sub: Subscription,
}

/// How many of the matching rows a mode lists: every tab, [`VISIBLE`] of anything else.
fn listed(mode: PaletteMode, matching: usize) -> usize {
    if mode == PaletteMode::Tabs { matching } else { matching.min(VISIBLE) }
}

/// How well `label` matches the lowercase query `q`: a substring (earlier is better), else its letters in order (fewer gaps is
/// better); `None` when it does not match.
pub(crate) fn score(label: &str, q: &str) -> Option<i32> {
    if q.is_empty() {
        return Some(0);
    }
    let l = label.to_lowercase();
    if let Some(i) = l.find(q) {
        return Some(100 - i as i32);
    }
    // subsequence match ("ykl" → "yeni klasör")
    let mut it = l.chars();
    let mut gaps = 0;
    for qc in q.chars() {
        let mut found = false;
        for c in it.by_ref() {
            if c == qc {
                found = true;
                break;
            }
            gaps += 1;
        }
        if !found {
            return None;
        }
    }
    Some(50 - gaps)
}

fn as_path(q: &str) -> Option<PathBuf> {
    let t = q.trim().trim_matches('"');
    let looks = t.len() >= 2 && (t.as_bytes()[1] == b':' || t.starts_with("\\\\") || t.starts_with('%'));
    looks.then(|| PathBuf::from(t))
}

/// The rows matching `query`, best first. Commands: every one while nothing is typed; past folders join them once something is
/// typed (at most [`FOLDERS_AMONG_COMMANDS`], a command first when both match as well). Folder mode: past folders only.
/// The tabs matching `q` (lower case), best first; every tab, open ones first, while nothing is typed.
fn filter_tabs(tabs: &[TabEntry], q: &str) -> Vec<Row> {
    let mut by: Vec<(i32, usize)> = tabs
        .iter()
        .enumerate()
        .filter_map(|(i, t)| {
            let s = score(&t.label, q).map(|s| s + 10).into_iter().chain(score(&t.detail, q)).max()?;
            Some((s - if t.closed { 1 } else { 0 }, i))
        })
        .collect();
    by.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    by.into_iter().map(|(_, i)| Row::Tab(i)).collect()
}

fn filter(mode: PaletteMode, cmds: &[Cmd], folders: &[crate::folder_history::Visit], query: &str) -> Vec<Row> {
    let q = query.trim().to_lowercase();
    let cmds = if matches!(mode, PaletteMode::Folders | PaletteMode::Tabs) { &[][..] } else { cmds };
    let mut by_cmd: Vec<(i32, usize)> = cmds.iter().enumerate().filter_map(|(i, c)| score(c.label, &q).map(|s| (s, i))).collect();
    by_cmd.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let by_folder: Vec<(i32, PathBuf)> = match mode {
        PaletteMode::Commands { with_folders: true } if !q.is_empty() => {
            crate::folder_history::rank(folders, &q).into_iter().take(FOLDERS_AMONG_COMMANDS).map(|(s, v)| (s, v.path.clone())).collect()
        }
        PaletteMode::Commands { .. } => Vec::new(),
        PaletteMode::Folders => crate::folder_history::rank(folders, &q).into_iter().map(|(s, v)| (s, v.path.clone())).collect(),
        PaletteMode::Tabs => Vec::new(),
    };
    // both lists are best first: merge them by how well they match
    let mut out = Vec::with_capacity(by_cmd.len() + by_folder.len());
    let (mut c, mut f) = (by_cmd.into_iter().peekable(), by_folder.into_iter().peekable());
    loop {
        let take_cmd = match (c.peek(), f.peek()) {
            (Some(a), Some(b)) => a.0 >= b.0,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => break,
        };
        if take_cmd {
            out.extend(c.next().map(|(_, i)| Row::Cmd(i)));
        } else {
            out.extend(f.next().map(|(_, p)| Row::Folder(p)));
        }
    }
    out
}

impl CommandPalette {
    /// `folders`: the remembered folders, most recent first (`folder_history::snapshot`); unused when the mode lists none.
    pub fn new(mode: PaletteMode, folders: Vec<crate::folder_history::Visit>, tabs: Vec<TabEntry>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let hint = match mode {
            PaletteMode::Commands { .. } => crate::t!("Komut ara veya yol yaz…"),
            PaletteMode::Folders => crate::t!("Geçmiş klasörlerde ara…"),
            PaletteMode::Tabs => crate::t!("Sekmelerde ara…"),
        };
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(hint));
        let sub = cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, _, cx| match ev {
            InputEvent::Change => this.refilter(cx),
            InputEvent::PressEnter { .. } => this.run_selected(cx),
            _ => {}
        });
        input.update(cx, |s, cx| s.focus(window, cx));
        let cmds = if matches!(mode, PaletteMode::Folders | PaletteMode::Tabs) { Vec::new() } else { commands() };
        let mut this = Self { input, mode, cmds, folders, tabs, filtered: Vec::new(), path: None, selected: 0, scroll: ScrollHandle::new(), _sub: sub };
        this.refilter(cx);
        this
    }

    fn refilter(&mut self, cx: &mut Context<Self>) {
        let raw = self.input.read(cx).value().to_string();
        // (the tab search takes no typed path)
        self.path = as_path(&raw).filter(|_| self.mode != PaletteMode::Tabs).map(|_| PathBuf::from(raw.trim().trim_matches('"')));
        self.filtered = if self.mode == PaletteMode::Tabs { filter_tabs(&self.tabs, &raw.trim().to_lowercase()) } else { filter(self.mode, &self.cmds, &self.folders, &raw) };
        self.selected = 0;
        self.scroll.set_offset(point(px(0.), px(0.)));
        cx.notify();
    }

    /// The rows drawn (and walked by Up/Down): the path row and the listed others (`listed`).
    fn rows(&self) -> usize {
        listed(self.mode, self.filtered.len()) + usize::from(self.path.is_some())
    }

    fn run_selected(&mut self, cx: &mut Context<Self>) {
        let mut ix = self.selected;
        if let Some(p) = &self.path {
            if ix == 0 {
                cx.emit(PaletteEvent::Navigate(p.clone()));
                return;
            }
            ix -= 1;
        }
        match self.filtered.get(ix) {
            Some(Row::Cmd(ci)) => cx.emit(PaletteEvent::Run((self.cmds[*ci].action)())),
            Some(Row::Folder(p)) => cx.emit(PaletteEvent::Navigate(p.clone())),
            Some(Row::Tab(ti)) => {
                let t = &self.tabs[*ti];
                cx.emit(PaletteEvent::Tab { closed: t.closed, id: t.id })
            }
            None => {}
        }
    }

    fn row(&self, ix: usize, icon: L, label: String, keys: SharedString, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let sel = ix == self.selected;
        h_flex()
            .id(("pal", ix))
            .h(px(ROW_H))
            .flex_shrink_0()
            .px_3()
            .gap_3()
            .items_center()
            .rounded(theme::RADIUS_SM)
            .text_sm()
            .when(sel, |d| d.bg(p.selection))
            .when(!sel, |d| d.hover(|s| s.bg(p.hover)))
            .child(Icon::new(icon).size(px(16.)).text_color(if sel { p.accent } else { p.text_secondary }))
            .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(label))
            // (a folder's full path here can be long: it gives way first)
            .when(!keys.is_empty(), |d| {
                d.child(
                    div()
                        .max_w(px(320.))
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_xs()
                        .text_color(p.text_tertiary)
                        .child(keys),
                )
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.selected = ix;
                this.run_selected(cx);
            }))
            .into_any_element()
    }
}

impl Render for CommandPalette {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::for_window(window);
        // (as high as `VISIBLE` rows at most: a longer list scrolls, and the row the keyboard is on is kept in view)
        let mut list = v_flex()
            .id("palette-list")
            .gap(px(ROW_GAP))
            .max_h(px(VISIBLE as f32 * ROW_H + (VISIBLE - 1) as f32 * ROW_GAP))
            .overflow_y_scroll()
            .track_scroll(&self.scroll);
        let mut ix = 0;
        if let Some(path) = self.path.clone() {
            list = list.child(self.row(ix, L::FolderOpen, format!("{} {}", crate::t!("Git:"), path.display()), "Enter".into(), &p, cx));
            ix += 1;
        }
        let visible: Vec<Row> = self.filtered.iter().take(listed(self.mode, self.filtered.len())).cloned().collect();
        let scrolls = visible.len() > VISIBLE;
        for r in visible {
            let (icon, label, keys): (L, String, SharedString) = match r {
                Row::Cmd(ci) => {
                    let c = &self.cmds[ci];
                    (c.icon, c.label.to_string(), c.keys.into())
                }
                Row::Folder(path) => (
                    L::Folder,
                    rf_core::history::display_name(&path),
                    path.to_string_lossy().into_owned().into(),
                ),
                // (a closed tab says so before its place)
                Row::Tab(ti) => {
                    let t = &self.tabs[ti];
                    let detail = if t.closed { format!("{} · {}", crate::t!("Kapatıldı"), t.detail) } else { t.detail.clone() };
                    (if t.closed { L::RotateCcw } else { L::PanelTop }, t.label.clone(), detail.into())
                }
            };
            list = list.child(self.row(ix, icon, label, keys, &p, cx));
            ix += 1;
        }
        // scrim + card
        div()
            .id("palette-scrim")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .bg(hsla(0., 0., 0., 0.25))
            // (the wheel over the palette is the palette's: at the end of its list it went on to the folder behind)
            .occlude()
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.emit(PaletteEvent::Dismiss)))
            .child(
                v_flex()
                    .id("palette")
                    .absolute()
                    .top(px(72.))
                    .left(relative(0.5))
                    .ml(px(-290.))
                    .w(px(580.))
                    .p_2()
                    .gap_2()
                    .rounded(theme::RADIUS)
                    .bg(p.card_bg)
                    .border_1()
                    .border_color(p.border)
                    .shadow_lg()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                        let n = this.rows();
                        match ev.keystroke.key.as_str() {
                            "down" if n > 0 => {
                                this.selected = (this.selected + 1) % n;
                                this.scroll.scroll_to_item(this.selected);
                                cx.stop_propagation();
                                cx.notify();
                            }
                            "up" if n > 0 => {
                                this.selected = (this.selected + n - 1) % n;
                                this.scroll.scroll_to_item(this.selected);
                                cx.stop_propagation();
                                cx.notify();
                            }
                            "escape" => {
                                cx.stop_propagation();
                                cx.emit(PaletteEvent::Dismiss);
                            }
                            _ => {}
                        }
                    }))
                    .child(
                        div()
                            .h(px(38.))
                            .px_2()
                            .flex()
                            .items_center()
                            .rounded(theme::RADIUS_SM)
                            .bg(p.hover)
                            .child(
                                Input::new(&self.input)
                                    .appearance(false)
                                    .prefix(
                                        Icon::new(if self.mode == PaletteMode::Folders { L::Clock } else { L::Command })
                                            .size(px(14.))
                                            .text_color(p.text_secondary),
                                    ),
                            ),
                    )
                    .child(div().relative().child(list).when(scrolls, |d| {
                        d.child(gpui_kit::component::scroll::Scrollbar::vertical(&self.scroll).id("palette-scrollbar").mode(gpui_kit::component::scroll::ScrollbarMode::Always))
                    }))
                    .when(self.rows() == 0, |d| {
                        let none = match self.mode {
                            PaletteMode::Commands { .. } => crate::t!("Eşleşen komut yok"),
                            PaletteMode::Folders if self.folders.is_empty() => crate::t!("Henüz gezilen klasör yok"),
                            PaletteMode::Folders => crate::t!("Eşleşen klasör yok"),
                            PaletteMode::Tabs => crate::t!("Eşleşen sekme yok"),
                        };
                        d.child(div().p_3().text_sm().text_color(p.text_secondary).child(none))
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{PaletteMode, Row, as_path, commands, filter, score};

    /// The tab search lists every tab that matches; commands and folders keep to what shows at once (issue 79).
    #[test]
    fn the_tab_search_lists_every_tab() {
        use super::{VISIBLE, listed};
        assert_eq!(listed(PaletteMode::Tabs, 40), 40);
        assert_eq!(listed(PaletteMode::Tabs, 3), 3);
        assert_eq!(listed(PaletteMode::Folders, 40), VISIBLE);
        assert_eq!(listed(PaletteMode::Commands { with_folders: true }, 40), VISIBLE);
    }

    #[test]
    fn the_tab_search_finds_a_tab_by_its_name_or_place_open_ones_first() {
        use super::{TabEntry, filter_tabs};
        let t = |label: &str, detail: &str, closed: bool, id: usize| TabEntry { label: label.into(), detail: detail.into(), closed, id };
        let tabs = vec![t("Downloads", "C:/Users/u/Downloads", false, 0), t("Projects", "D:/Work/Projects", false, 1), t("Downloads", "E:/Old/Downloads", true, 0)];
        assert_eq!(filter_tabs(&tabs, ""), vec![Row::Tab(0), Row::Tab(1), Row::Tab(2)], "all, open ones first");
        assert_eq!(filter_tabs(&tabs, "down"), vec![Row::Tab(0), Row::Tab(2)], "the open one before the closed one of the same name");
        assert_eq!(filter_tabs(&tabs, "work"), vec![Row::Tab(1)], "by its place too");
        assert!(filter_tabs(&tabs, "zzz").is_empty());
    }
    use crate::folder_history::Visit;
    use std::path::PathBuf;

    fn visits() -> Vec<Visit> {
        [r"D:\Projeler\rustyfiles", r"C:\Users\me\Downloads", r"C:\Windows"]
            .iter()
            .enumerate()
            .map(|(i, p)| Visit { path: PathBuf::from(p), last_ms: 10 - i as i64, count: 1 })
            .collect()
    }

    fn folders(rows: &[Row]) -> Vec<String> {
        rows.iter().filter_map(|r| if let Row::Folder(p) = r { Some(p.to_string_lossy().into_owned()) } else { None }).collect()
    }

    #[test]
    fn folders_join_the_commands_only_once_typed_and_only_when_allowed() {
        let (cmds, v) = (commands(), visits());
        let on = PaletteMode::Commands { with_folders: true };
        assert!(folders(&filter(on, &cmds, &v, "")).is_empty(), "nothing typed: the commands as before");
        assert_eq!(folders(&filter(on, &cmds, &v, "downl")), [r"C:\Users\me\Downloads"]);
        assert!(folders(&filter(PaletteMode::Commands { with_folders: false }, &cmds, &v, "downl")).is_empty());
    }

    #[test]
    fn folder_mode_lists_past_folders_only_most_recent_first() {
        let (cmds, v) = (commands(), visits());
        let all = filter(PaletteMode::Folders, &cmds, &v, "");
        assert_eq!(folders(&all), [r"D:\Projeler\rustyfiles", r"C:\Users\me\Downloads", r"C:\Windows"]);
        assert_eq!(all.len(), 3, "no commands");
        assert_eq!(folders(&filter(PaletteMode::Folders, &cmds, &v, "rusty")), [r"D:\Projeler\rustyfiles"]);
    }

    #[test]
    fn fuzzy() {
        assert!(score("Yeni klasör", "klas").unwrap() > score("Yeni klasör", "ykl").unwrap());
        assert!(score("Yeni klasör", "xyz").is_none());
        assert_eq!(as_path(r"C:\Windows"), Some(PathBuf::from(r"C:\Windows")));
        assert!(as_path("yeni").is_none());
    }
}
