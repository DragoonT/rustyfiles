//! The desktop's two menus, built from the app's own Fluent menu (`fluent_menu`) with the shell's items loaded into "Show more
//! options" and "New" in the background, as the file list's menus do (`ctx_menu.rs`).
//!
//! The file list's helper that turns the shell's menu into our items is private to `ctx_menu.rs` and tied to the file list's view
//! (it hands "Rename" back to it). The few lines here do the same job for this view over the same `rf_win::ctxmenu` calls; the list
//! of verbs that are hidden because we show them ourselves is the file list's.

use std::path::PathBuf;
use std::rc::Rc;

use futures::channel::oneshot;
use gpui_kit::assets::IconName as L;
use gpui_kit::*;
use rf_core::desktop::grid::IconSize;
use rf_core::desktop::items::Origin;
use rf_core::desktop::store::SortName;
use rf_win::ctxmenu::{MenuOutcome, ShellItem, ShellTarget};
use rf_win::fileops::Transfer;
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

use super::menupoint::Via;
use super::menu_plan::{BgCmd, BgCtx, ItemCmd, ItemCtx, Row, background_menu, item_menu, item_quick, paste_enabled};
use super::view::DesktopView;
use crate::fluent_menu::{self, Item, Menu, MenuIcon, MenuView, Pending, Quick, Sub};
use crate::t;

type Me = WeakEntity<DesktopView>;

/// Shell verbs shown as our own commands (or that make no sense here), hidden from "Show more options": `ctx_menu.rs`'s list.
const HIDDEN_VERBS: &[&str] = &[
    "open",
    "opennewwindow",
    "opennewtab",
    "opennewprocess",
    "explore",
    "cut",
    "copy",
    "paste",
    "pastelink",
    "delete",
    "rename",
    "properties",
    "link",
    "copyaspath",
    "pintohome",
    "pintohomefile",
    "windows.modernshare",
    "windows.share",
    "openas",
    "newfolder",
    "{9f156763-7844-4dc4-b2b1-901f640f5155}",
];

fn act(me: &Me, f: impl Fn(&mut DesktopView, &mut Window, &mut Context<DesktopView>) + 'static) -> impl Fn(&mut Window, &mut App) + 'static {
    let me = me.clone();
    move |w, cx| {
        let _ = me.update(cx, |v, cx| f(v, w, cx));
    }
}

fn icon(l: L) -> MenuIcon {
    MenuIcon::Lucide(l)
}

/// The menu entry of an item command.
fn item_of(me: &Me, cmd: ItemCmd) -> Item {
    match cmd {
        ItemCmd::Open => Item::action(t!("Aç"), icon(L::SquareArrowOutUpRight), act(me, |v, w, cx| v.open_selection(w, cx))).hint("Enter"),
        ItemCmd::OpenWith => Item::action(t!("Birlikte aç…"), icon(L::AppWindow), act(me, |v, _, cx| v.open_with_selection(cx))),
        ItemCmd::CreateShortcut => Item::action(t!("Kısayol oluştur"), icon(L::Link), act(me, |v, _, cx| v.create_shortcuts(cx))),
        ItemCmd::EmptyBin => Item::action(t!("Geri Dönüşüm Kutusu'nu boşalt"), icon(L::Trash), act(me, |v, _, cx| v.empty_bin(cx))),
        ItemCmd::CopyPath => Item::action(t!("Öğe yolunu kopyala"), icon(L::ClipboardCopy), act(me, |v, _, cx| v.copy_paths_as_text(cx))).hint("Ctrl+Shift+C"),
        ItemCmd::Properties => Item::action(t!("Özellikler"), icon(L::Wrench), act(me, |v, _, cx| v.properties(cx))).hint("Alt+Enter"),
    }
}

fn cursor() -> (i32, i32) {
    let mut pt = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut pt);
    }
    (pt.x, pt.y)
}

impl DesktopView {
    /// The menu of the selected items, at the pointer. WHICH commands it holds is `menu_plan::item_menu`'s (tested); this draws them.
    pub(super) fn show_item_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ids = self.selection_ids();
        if ids.is_empty() {
            return;
        }
        let me = cx.entity().downgrade();
        let user_only = ids.iter().all(|i| self.item(i).is_some_and(|it| it.origin == Origin::User));
        let single = ids.len() == 1;
        let kinds = self.kinds_of(&ids);
        let bin_full = ids.iter().any(|i| self.item(i).is_some_and(|it| it.recycle_full == Some(true)));
        let ctx = ItemCtx { read_only: self.read_only(), bin_full, bin_locked: self.is_scratch() };
        let items: Vec<Item> = item_menu(&kinds, ctx)
            .into_iter()
            .map(|row| match row {
                Row::Sep => Item::Separator,
                Row::More => Item::Sub { label: t!("Diğer seçenekleri göster").into(), icon: icon(L::Ellipsis), items: Sub::Pending { key: "more", fallback: None } },
                Row::Cmd(cmd, enabled) => item_of(&me, cmd).enabled(enabled),
            })
            .collect();
        let q = item_quick(&kinds, ctx);
        let quick = vec![
            Quick::new(L::Scissors, t!("Kes  (Ctrl+X)"), q.cut, act(&me, |v, _, cx| v.copy_selection(Transfer::Move, cx))),
            Quick::new(L::Copy, t!("Kopyala  (Ctrl+C)"), q.copy, act(&me, |v, _, cx| v.copy_selection(Transfer::Copy, cx))),
            Quick::new(L::TextCursorInput, t!("Yeniden adlandır  (F2)"), q.rename, act(&me, |v, w, cx| v.begin_rename(w, cx))),
            Quick::new(L::Trash, t!("Sil  (Del)"), q.delete, act(&me, |v, w, cx| v.delete_selection(false, w, cx))),
        ];
        let view = fluent_menu::show(Menu { quick, items }, window.mouse_position(), window, cx);

        // the shell's own commands for the selection: one origin only (they must share a parent folder)
        let first = ids.first().and_then(|i| self.item(i)).cloned();
        // (a scratch desktop's Recycle Bin is the real one: Windows' own menu for it, with its Empty command, is not offered there)
        let target = match first.filter(|_| !self.read_only()) {
            Some(it) if it.is_virtual() && self.is_scratch() => None,
            Some(it) if it.is_virtual() => single.then(|| ShellTarget::One(PathBuf::from(it.id.as_str()))),
            Some(it) if user_only || ids.iter().all(|i| self.item(i).is_some_and(|x| x.origin == it.origin)) => {
                let paths = self.paths_of(&ids);
                match paths.len() {
                    0 => None,
                    1 => Some(ShellTarget::One(paths[0].clone())),
                    _ => Some(ShellTarget::Items(paths)),
                }
            }
            _ => None,
        };
        match target {
            Some(t) => self.load_shell(view.downgrade(), t, false, cx),
            None => {
                let none = self.shell_items_off();
                view.update(cx, |m, cx| m.set_pending("more", none, cx));
            }
        }
    }

    /// What "Show more options" holds when the shell's own commands are not offered: one line that says why (not "could not be loaded",
    /// which would read as a fault).
    fn shell_items_off(&self) -> Pending {
        let why = if self.read_only() { t!("Salt okunur önizlemede kapalı") } else { t!("Bu öğeler için kullanılamıyor") };
        Pending::Ready(Rc::new(vec![Item::action(why, MenuIcon::None, |_, _| {}).enabled(false)]))
    }

    /// The menu of the empty desktop, at the pointer. WHICH commands it holds is `menu_plan::background_menu`'s (tested).
    pub(super) fn show_background_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let me = cx.entity().downgrade();
        let (size, sort, align, auto) = {
            let st = self.store.get();
            (st.icon_size_step(), st.sort, st.align_to_grid, st.auto_arrange)
        };
        let read_only = self.read_only();
        let (undo, redo) = crate::undo::next_labels();
        let ctx = BgCtx {
            read_only,
            clipboard_files: rf_win::clipboard::has_files(),
            undo: undo.map(str::to_string),
            redo: redo.map(str::to_string),
            has_folder: self.backend.as_ref().and_then(|b| b.target_dir()).is_some(),
            icons_forced_hidden: super::choices::hidden_by_settings(),
        };
        let size_item = |label: &str, s: IconSize| {
            Item::action(label.to_string(), MenuIcon::None, act(&me, move |v, _, cx| v.set_icon_size(s, cx))).checked(size == s)
        };
        let by = |label: &str, k: SortName| {
            Item::action(label.to_string(), MenuIcon::None, act(&me, move |v, _, cx| {
                let desc = v.store.get().sort.descending;
                v.sort_by(k, desc, cx)
            }))
            .checked(sort.key == k)
        };
        let dir = |label: &str, desc: bool| {
            Item::action(label.to_string(), MenuIcon::None, act(&me, move |v, _, cx| {
                let k = v.store.get().sort.key;
                v.sort_by(k, desc, cx)
            }))
            .checked(sort.descending == desc)
        };
        let shown = !super::choices::menu_hidden();
        let items: Vec<Item> = background_menu(&ctx)
            .into_iter()
            .map(|row| match row {
                Row::Sep => Item::Separator,
                Row::More => Item::Sub { label: t!("Diğer seçenekleri göster").into(), icon: icon(L::Ellipsis), items: Sub::Pending { key: "more", fallback: None } },
                Row::Cmd(cmd, enabled) => match cmd {
                    BgCmd::IconSize => Item::sub(
                        t!("Simge boyutu"),
                        icon(L::LayoutGrid),
                        vec![size_item(t!("Küçük"), IconSize::Small), size_item(t!("Orta boy"), IconSize::Medium), size_item(t!("Büyük"), IconSize::Large)],
                    ),
                    BgCmd::AlignToGrid => Item::action(t!("Simgeleri ızgaraya hizala"), icon(L::LayoutGrid), act(&me, move |v, _, cx| v.set_align(!align, cx))).checked(align),
                    BgCmd::AutoArrange => Item::action(t!("Simgeleri otomatik düzenle"), icon(L::ArrowDownUp), act(&me, move |v, _, cx| v.set_auto_arrange_mode(!auto, cx))).checked(auto),
                    BgCmd::ShowIcons => Item::action(t!("Masaüstü simgelerini göster"), icon(L::Eye), act(&me, move |v, _, cx| v.set_show_icons(!shown, cx))).checked(shown && enabled),
                    BgCmd::Sort => Item::sub(
                        t!("Sırala"),
                        icon(L::ArrowUpDown),
                        vec![
                            by(t!("Ad"), SortName::Name),
                            by(t!("Değiştirme tarihi"), SortName::Modified),
                            by(t!("Tür"), SortName::Type),
                            by(t!("Boyut"), SortName::Size),
                            Item::Separator,
                            dir(t!("Artan"), false),
                            dir(t!("Azalan"), true),
                        ],
                    ),
                    BgCmd::Refresh => Item::action(t!("Yenile"), icon(L::RotateCw), act(&me, |v, _, cx| v.refresh(cx))).hint("Ctrl+R"),
                    // the read-only preview creates nothing: "New" is there but off, and the shell's own commands (which can write to the
                    // Desktop: archive, send to, templates) are not offered at all
                    BgCmd::New if read_only => Item::action(t!("Yeni"), icon(L::CirclePlus), |_, _| {}).enabled(false),
                    BgCmd::New => Item::Sub { label: t!("Yeni").into(), icon: icon(L::CirclePlus), items: Sub::Pending { key: "new", fallback: Some(Rc::new(own_new_items(&me))) } },
                    BgCmd::PasteShortcut => Item::action(t!("Kısayol yapıştır"), icon(L::Link), act(&me, |v, _, cx| v.paste_shortcuts(cx))),
                    BgCmd::Undo(what) => Item::action(format!("{}: {what}", t!("Geri al")), icon(L::Undo2), act(&me, |_, _, cx| crate::undo::undo(cx))).hint("Ctrl+Z"),
                    BgCmd::Redo(what) => Item::action(format!("{}: {what}", t!("Yinele")), icon(L::Redo2), act(&me, |_, _, cx| crate::undo::redo(cx))).hint("Ctrl+Y"),
                    BgCmd::OpenInRustyfiles => Item::action(t!("rustyfiles'ta aç"), icon(L::FolderOpen), act(&me, |v, _, cx| v.open_desktop_folder(cx))),
                    BgCmd::OpenTerminal => Item::action(t!("Terminalde aç"), icon(L::SquareTerminal), act(&me, |v, _, _| v.open_terminal_here())),
                    BgCmd::DisplaySettings => Item::action(t!("Görüntü ayarları"), icon(L::Monitor), act(&me, |v, _, _| v.open_settings_page("ms-settings:display"))),
                    BgCmd::Personalize => Item::action(t!("Kişiselleştir"), icon(L::Palette), act(&me, |v, _, _| v.open_settings_page("ms-settings:personalization"))),
                }
                .enabled(enabled),
            })
            .collect();
        let quick = vec![Quick::new(L::ClipboardPaste, t!("Yapıştır  (Ctrl+V)"), paste_enabled(&ctx), act(&me, |v, w, cx| v.paste(Via::Menu, w, cx)))];
        let view = fluent_menu::show(Menu { quick, items }, window.mouse_position(), window, cx);
        match self.backend.as_ref().and_then(|b| b.target_dir()).filter(|_| !read_only) {
            Some(dir) => self.load_shell(view.downgrade(), ShellTarget::Background(dir), true, cx),
            None => {
                let none = self.shell_items_off();
                view.update(cx, |m, cx| m.set_pending("more", none, cx));
            }
        }
    }

    /// Fills "Show more options" (and, for the background, the shell's templates under "New") when the shell has answered.
    fn load_shell(&mut self, menu: WeakEntity<MenuView>, target: ShellTarget, background: bool, cx: &mut Context<Self>) {
        let (tx, rx) = oneshot::channel();
        rf_win::sta::run(move |owner| {
            let _ = tx.send(rf_win::ctxmenu::build(owner, &target, false).map_err(|e| format!("{e:#}")));
        });
        let me = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result = rx.await.unwrap_or_else(|_| Err("cancelled".into()));
            cx.update(|cx| {
                let (more, new) = match result {
                    Ok((session, shell)) => split_shell_items(&me, session, shell, background, cx),
                    Err(e) => {
                        log::warn!("desktop shell menu: {e}");
                        (Pending::Failed, Pending::Failed)
                    }
                };
                let inline = !cx.global::<crate::settings::Settings>().menu.shell_submenu;
                if let Some(m) = menu.upgrade() {
                    m.update(cx, |m, cx| {
                        m.set_pending("more", more, cx);
                        if background {
                            m.set_pending("new", new, cx);
                        }
                        if inline {
                            m.inline_pending("more", cx);
                        }
                    });
                }
            });
        })
        .detach();
    }
}

fn own_new_items(me: &Me) -> Vec<Item> {
    vec![
        Item::action(t!("Klasör"), MenuIcon::Tinted(L::Folder, crate::theme::kind_color(crate::icons::Category::Folder, true)), act(me, |v, _, cx| v.new_item(true, Via::Menu, cx))).hint("Ctrl+Shift+N"),
        Item::action(t!("Metin belgesi"), icon(L::FileText), act(me, |v, _, cx| v.new_item(false, Via::Menu, cx))),
    ]
}

/// The shell's menu as our two lazy lists: what is left after hiding our own commands, and the "New" templates.
fn split_shell_items(me: &Me, session: u64, shell: Vec<ShellItem>, background: bool, cx: &mut App) -> (Pending, Pending) {
    let hidden = |it: &ShellItem| !it.verb.is_empty() && HIDDEN_VERBS.contains(&it.verb.to_ascii_lowercase().as_str());
    let has_child_verb = |it: &ShellItem, verb: &str| it.children.iter().any(|c| c.verb.eq_ignore_ascii_case(verb));
    let mut more = Vec::new();
    let mut new: Option<Vec<ShellItem>> = None;
    for it in shell {
        if !it.children.is_empty() && has_child_verb(&it, "NewFolder") {
            new = Some(it.children.clone());
            continue;
        }
        // "Open with" has its own place in the file list's menu; here the shell's version stays inside "more options"
        if hidden(&it) {
            continue;
        }
        more.push(it);
    }
    let more = Pending::Ready(Rc::new(to_items(me, session, &tidy(more), cx)));
    let new = match new.filter(|_| background) {
        Some(v) => {
            // ours for Folder and Text (their rename box opens), the shell for every other template
            let rest: Vec<ShellItem> = v.into_iter().filter(|c| !c.verb.eq_ignore_ascii_case("NewFolder") && !c.verb.eq_ignore_ascii_case(".txt")).collect();
            let mut items = own_new_items(me);
            let rest = to_items(me, session, &tidy(rest), cx);
            if !rest.is_empty() {
                items.push(Item::Separator);
                items.extend(rest);
            }
            Pending::Ready(Rc::new(items))
        }
        None => Pending::Failed,
    };
    (more, new)
}

/// No leading, trailing or doubled separators.
fn tidy(items: Vec<ShellItem>) -> Vec<ShellItem> {
    let mut out: Vec<ShellItem> = Vec::with_capacity(items.len());
    for it in items {
        if it.separator && out.last().is_none_or(|l| l.separator) {
            continue;
        }
        out.push(it);
    }
    while out.last().is_some_and(|l| l.separator) {
        out.pop();
    }
    out
}

fn to_items(me: &Me, session: u64, items: &[ShellItem], cx: &mut App) -> Vec<Item> {
    items
        .iter()
        .map(|it| {
            if it.separator {
                return Item::Separator;
            }
            let icon = it.icon.as_ref().and_then(|i| fluent_menu::shell_icon(i, cx)).map(MenuIcon::Image).unwrap_or(MenuIcon::None);
            if !it.children.is_empty() {
                return Item::Sub { label: it.label.clone().into(), icon, items: Sub::Items(Rc::new(to_items(me, session, &tidy(it.children.clone()), cx))) };
            }
            let (id, me) = (it.id, me.clone());
            let mut item = Item::action(it.label.clone(), icon, move |_, cx| invoke_shell(&me, session, id, cx)).enabled(it.enabled);
            if let Some(h) = &it.hint {
                item = item.hint(h.clone());
            }
            item
        })
        .collect()
}

fn invoke_shell(me: &Me, session: u64, id: u32, cx: &mut App) {
    let (tx, rx) = oneshot::channel();
    rf_win::sta::run(move |owner| {
        let r = rf_win::ctxmenu::invoke(owner, session, id, cursor());
        if let Err(e) = &r {
            log::warn!("desktop shell command: {e:#}");
        }
        let _ = tx.send(r.ok());
    });
    let me = me.clone();
    cx.spawn(async move |cx| {
        let outcome = rx.await;
        let _ = me.update(cx, |v, cx| match outcome {
            Ok(Some(MenuOutcome::Rename)) => v.rename_from_menu(cx),
            // what the shell did (a deletion, a new item) is seen by the folder watcher; a list again does no harm
            Ok(Some(MenuOutcome::Invoked(_))) => v.refresh(cx),
            _ => {}
        });
    })
    .detach();
}
