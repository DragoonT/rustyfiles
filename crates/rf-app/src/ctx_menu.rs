//! The file-list context menu (Files style): quick action buttons, our own commands, and the
//! shell's items loaded in the background into "Open with", "New" and "Show more options".
//! Shift+right-click still opens the classic Explorer menu.

use std::path::PathBuf;
use std::rc::Rc;

use futures::channel::oneshot;
use gpui_kit::assets::IconName as L;
use gpui_kit::*;
use rf_core::{Location, SortDir, SortKey};
use rf_win::ctxmenu::{MenuOutcome, ShellItem, ShellTarget};
use rf_win::fileops::Transfer;
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

use crate::fluent_menu::{self, Item, Menu, MenuIcon, Pending, Quick, Sub};
use crate::folder_view::{FolderEvent, FolderView, GroupBy, ViewMode};
use crate::t;

/// Shell verbs we show as our own commands (or that make no sense here), hidden from
/// "Show more options" — the same idea as Files' filter list.
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
    // Windows Terminal's "Open in Terminal" (we have our own)
    "{9f156763-7844-4dc4-b2b1-901f640f5155}",
];

type Me = WeakEntity<FolderView>;

/// A menu action that runs on the folder view.
fn act(me: &Me, f: impl Fn(&mut FolderView, &mut Window, &mut Context<FolderView>) + 'static) -> impl Fn(&mut Window, &mut App) + 'static {
    let me = me.clone();
    move |w, cx| {
        let _ = me.update(cx, |v, cx| f(v, w, cx));
    }
}

fn icon(l: L) -> MenuIcon {
    MenuIcon::Lucide(l)
}

fn cursor() -> (i32, i32) {
    let mut pt = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut pt);
    }
    (pt.x, pt.y)
}

impl FolderView {
    /// Opens our context menu for the selection (or the folder background) at the mouse.
    pub fn show_context_menu(&mut self, background: bool, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.location(), rf_core::history::Location::Shell { .. }) {
            if shift_down() {
                self.show_shell_folder_menu(background, cx);
            } else {
                self.show_shell_fluent_menu(background, window, cx);
            }
            return;
        }
        let shift = unsafe {
            use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_SHIFT};
            GetKeyState(VK_SHIFT.0 as i32) < 0
        };
        if shift {
            // power users: the full classic Explorer menu (extended verbs)
            self.show_native_menu(background, cx);
            return;
        }
        let Some(dir) = self.current_dir() else { return };
        let items = if background { Vec::new() } else { self.selection_paths() };
        let background = background || items.is_empty();
        let me = cx.entity().downgrade();
        let menu = if background { self.background_menu(&me, &dir, cx) } else { self.items_menu(&me, &dir, &items, cx) };
        // "Show more options": held back where it came back empty the last time for the same kind of item, so that it does not show
        // for a moment and go (issue 368); it is added when something comes after all. (Not with the shell's items in the menu
        // itself: there is no row to hold back.) It is taken out before the menu's window is made: taken out after, the window
        // opened with its height and then shrank.
        let more_key = more_kind(background, &items);
        let inline_more = !cx.global::<crate::settings::Settings>().menu.shell_submenu;
        let menu_view = if !inline_more && more_was_empty(&more_key) {
            fluent_menu::show_context_holding(menu, window.mouse_position(), "more", window, cx)
        } else {
            fluent_menu::show_context(menu, window.mouse_position(), window, cx)
        };

        // Shell items in the background: QueryContextMenu can take a second with slow extensions.
        let target = if background { ShellTarget::Background(dir.clone()) } else { ShellTarget::Items(items.clone()) };
        // The user's own programs for the first selected file's type (Settings > File types): the first rows of "Open with". Their
        // icons are read on the worker, with the shell's items: never on the UI thread.
        let own: Vec<String> = if background { Vec::new() } else { items.first().map(|f| rf_win::openrules::programs_for(f)).unwrap_or_default() };
        // "Send to" (issue 117): its entries and their icons are read on the worker too, only when the menu has the row
        let send_to = !background && send_to_fits(&items, cx);
        let drives = if send_to { self.send_to_drives(cx) } else { Vec::new() };
        let send_items = items.clone();
        // the packaged apps' commands of Windows 11's new menu (issue 107), read on the worker with their icons
        let pkg_items = if background { vec![dir.clone()] } else { items.clone() };
        let (tx, rx) = oneshot::channel();
        rf_win::sta::run(move |owner| {
            let packaged = packaged_commands(rf_win::pkgverbs::for_items(&pkg_items, background));
            let own: Vec<OwnProgram> = own
                .into_iter()
                .map(|program| {
                    let icon = rf_win::thumbs::get_image(std::path::Path::new(&program), 32, rf_win::thumbs::Kind::Icon).ok();
                    (program, icon)
                })
                .collect();
            let targets: Vec<SendTarget> = if send_to {
                rf_win::sendto::targets()
                    .into_iter()
                    .map(|t| {
                        let icon = rf_win::thumbs::get_image(&t.path, 32, rf_win::thumbs::Kind::Icon).ok();
                        (t, icon)
                    })
                    .collect()
            } else {
                Vec::new()
            };
            let _ = tx.send((rf_win::ctxmenu::build(owner, &target, false).map_err(|e| format!("{e:#}")), own, targets, packaged));
        });
        let menu_weak = menu_view.downgrade();
        cx.spawn(async move |_, cx| {
            let (result, own, targets, packaged) = rx.await.unwrap_or_else(|_| (Err("cancelled".into()), Vec::new(), Vec::new(), Vec::new()));
            let _ = cx.update(|cx| {
                let (more, open_with, new) = match result {
                    Ok((session, shell)) => {
                        let ours: Vec<String> = if send_to { targets.iter().map(|(t, _)| t.name.clone()).collect() } else { Vec::new() };
                        split_shell_items(&me, session, shell, background, &ours, cx)
                    }
                    Err(e) => {
                        log::warn!("shell menu: {e}");
                        (Pending::Failed, Pending::Failed, Pending::Failed)
                    }
                };
                let open_with = with_own_programs(&me, own, open_with);
                let more = with_packaged(packaged, more, cx.global::<crate::settings::Settings>().menu.prefer_classic_commands);
                if let Pending::Ready(v) = &more {
                    remember_more(more_key.clone(), v.is_empty());
                }
                let inline = !cx.global::<crate::settings::Settings>().menu.shell_submenu;
                if let Some(m) = menu_weak.upgrade() {
                    m.update(cx, |m, cx| {
                        m.set_pending("more", more, cx);
                        m.set_pending("openwith", open_with, cx);
                        m.set_pending("new", new, cx);
                        if send_to {
                            m.set_pending("sendto", send_to_items(targets, drives, send_items), cx);
                        }
                        // Settings: shell extensions listed in the menu itself, not in a submenu
                        if inline {
                            m.inline_pending("more", cx);
                        }
                    });
                }
            });
        })
        .detach();
    }

    fn quick_clipboard(&self, me: &Me, with_items: bool) -> Vec<Quick> {
        let paste_ok = rf_win::clipboard::can_paste();
        if !with_items {
            return vec![
                Quick::new(L::ClipboardPaste, t!("Yapıştır  (Ctrl+V)"), paste_ok, act(me, |v, w, cx| v.paste(w, cx))),
                Quick::new(L::Wrench, t!("Özellikler  (Alt+Enter)"), true, act(me, |v, _, cx| v.folder_properties(cx))),
            ];
        }
        vec![
            Quick::new(L::Scissors, t!("Kes  (Ctrl+X)"), true, act(me, |v, w, cx| v.copy_to_clipboard(Transfer::Move, w, cx))),
            Quick::new(L::Copy, t!("Kopyala  (Ctrl+C)"), true, act(me, |v, w, cx| v.copy_to_clipboard(Transfer::Copy, w, cx))),
            Quick::new(
                L::ClipboardPaste,
                t!("Yapıştır  (Ctrl+V)"),
                paste_ok,
                act(me, |v, w, cx| if v.selected_is_single_dir() { v.paste_into_selected(w, cx) } else { v.paste(w, cx) }),
            ),
            Quick::new(L::TextCursorInput, t!("Yeniden adlandır  (F2)"), true, act(me, |v, w, cx| v.rename_selection(w, cx))),
            Quick::new(L::Share2, t!("Paylaş"), true, act(me, |v, w, cx| v.share(w, cx))),
            Quick::new(L::Trash, t!("Sil  (Del)"), true, act(me, |v, w, cx| v.delete_selected(false, w, cx))),
            Quick::new(L::Wrench, t!("Özellikler  (Alt+Enter)"), true, act(me, |v, _, cx| v.properties(cx))),
        ]
    }

    fn items_menu(&self, me: &Me, dir: &std::path::Path, items: &[PathBuf], cx: &mut Context<Self>) -> Menu {
        let single_dir = items.len() == 1 && self.selected_is_single_dir();
        // one archive that opens like a folder: it can open in a new tab or window too (Discord, 2026-11-20)
        let single_archive = items.len() == 1
            && !single_dir
            && cx.global::<crate::settings::Settings>().browse_archives
            && items[0].exists()
            && crate::archive_view::is_browsable(&items[0]);
        let has_files = !self.selected_is_single_dir() || items.len() > 1;
        let target_dir = if single_dir { items[0].clone() } else { dir.to_path_buf() };
        let prefs = cx.global::<crate::settings::Settings>();
        let (mp, show_flatten, explorer_theme) = (prefs.menu.clone(), prefs.show_flatten, prefs.windows_file_explorer_theme);
        // Members of an archive (ZIP, 7z, RAR... opened like a folder) have no file of their own: the rows that need one are left out
        // (a shortcut, running as administrator, compressing, tags, the Shelf, the terminal...), and the few that make sense take an
        // extracted copy first (the desktop background, installing a font). Copying out, dragging out, renaming and deleting inside work.
        let in_archive = items.iter().any(|p| crate::archive_view::split(p).is_some() && !p.exists());
        let mut v: Vec<Item> = Vec::new();

        let open_icon = if explorer_theme {
            items
                .first()
                .and_then(|path| {
                    let parsing = path.to_string_lossy();
                    self.images.update(cx, |images, cx| images.shell_icon_ui(&parsing, 16, cx))
                })
                .map(MenuIcon::Image)
                .unwrap_or_else(|| {
                    if single_dir {
                        MenuIcon::Tinted(L::Folder, crate::theme::kind_color(crate::icons::Category::Folder, true))
                    } else {
                        let ext = items.first().and_then(|path| path.extension()).map(|ext| ext.to_string_lossy().to_lowercase()).unwrap_or_default();
                        MenuIcon::Lucide(crate::icons::icon_for(crate::icons::category_for_ext(&ext)))
                    }
                })
        } else {
            icon(L::SquareArrowOutUpRight)
        };
        v.push(Item::action(t!("Aç"), open_icon, act(me, |v, _, cx| v.open_selected(cx))).hint("Enter"));
        // the commands of the file's own type go here, right below Open (see `raise`)
        let mut type_at = v.len();
        if single_dir || single_archive {
            let opened = if single_archive { items[0].clone() } else { target_dir.clone() };
            let (a, b) = (opened.clone(), opened);
            if mp.open_in_new_tab {
                v.push(Item::action(t!("Yeni sekmede aç"), icon(L::SquarePlus), act(me, move |_, _, cx| {
                    cx.emit(FolderEvent::OpenInNewTab(Location::Path(a.clone())))
                })));
            }
            if mp.open_in_new_window {
                v.push(Item::action(t!("Yeni pencerede aç"), icon(L::AppWindow), act(me, move |_, _, cx| {
                    cx.emit(FolderEvent::OpenInNewWindow(Location::Path(b.clone())))
                })));
            }
            if self.dual {
                let c = target_dir.clone();
                v.push(Item::action(t!("Diğer panelde aç"), icon(L::Columns2), act(me, move |_, _, cx| {
                    cx.emit(FolderEvent::OpenInOtherPane(Location::Path(c.clone())))
                })));
            }
        }
        if has_files {
            let icon = icon(L::LayoutGrid);
            v.push(Item::Sub {
                label: t!("Şununla aç").into(),
                icon,
                items: Sub::Pending { key: "openwith", fallback: None },
                click: None,
            });
        }
        if explorer_theme && rf_win::clipboard::can_paste() {
            v.push(
                Item::action(
                    t!("Yapıştır"),
                    icon(L::ClipboardPaste),
                    act(me, |v, w, cx| if v.selected_is_single_dir() { v.paste_into_selected(w, cx) } else { v.paste(w, cx) }),
                )
                .hint("Ctrl+V"),
            );
        }
        // Explorer's "Send to" in the first menu (issue 117): its entries come with the shell's items (see `show_context_menu`)
        if send_to_fits(items, cx) {
            v.push(Item::Sub { label: t!("Gönder").into(), icon: icon(L::Send), items: Sub::Pending { key: "sendto", fallback: None }, click: None });
        }
        if self.is_search() && items.len() == 1 {
            let p = items[0].clone();
            v.push(Item::action(t!("Dosya konumunu aç"), icon(L::FolderOpen), act(me, move |v, _, cx| {
                if let (Some(parent), Some(name)) = (p.parent(), p.file_name()) {
                    v.set_reselect(name.to_string_lossy().into_owned());
                    v.navigate(Location::Path(parent.to_path_buf()), cx);
                }
            })));
        }
        v.push(Item::Separator);
        if mp.copy_path {
            v.push(Item::action(t!("Öğe yolunu kopyala"), icon(L::ClipboardCopy), act(me, |v, _, cx| v.copy_path(cx))).hint("Ctrl+Shift+C"));
        }
        if !in_archive {
            v.push(Item::action(t!("Çoğalt"), icon(L::CopyPlus), act(me, |v, _, cx| v.duplicate_selected(cx))).hint("Ctrl+Alt+D"));
        }
        if mp.create_shortcut && !in_archive {
            let link_items = items.to_vec();
            v.push(Item::action(t!("Kısayol oluştur"), icon(L::Link), move |_, _| create_shortcuts(link_items.clone())));
        }
        if !self.is_search() && !in_archive {
            let sel = items.to_vec();
            v.push(Item::action(t!("Seçimle klasör oluştur"), icon(L::FolderInput), act(me, move |v, _, cx| v.folder_with_selection(sel.clone(), cx))));
        }
        if single_dir && crate::archive_view::split(&target_dir).is_none() && !rf_win::ftp::is_ftp(&target_dir) {
            let d = target_dir.clone();
            v.push(Item::action(t!("Disk kullanımını göster"), icon(L::ChartPie), act(me, move |_, _, cx| {
                cx.emit(FolderEvent::OpenInNewTab(Location::DiskUsage(d.clone())))
            })));
        }
        if single_dir && mp.pin {
            let d = target_dir.clone();
            v.push(Item::action(t!("Kenar çubuğuna sabitle"), icon(L::Pin), act(me, move |v, _, cx| v.pin(d.clone(), cx))));
        }
        if single_dir && crate::archive_view::split(&target_dir).is_none() {
            let d = target_dir.clone();
            let label = match crate::aliases::name_of(&d) {
                Some(n) => format!("{} (@{n})…", t!("Takma adı düzenle")),
                None => format!("{}…", t!("Takma ad ver")),
            };
            v.push(Item::action(label, icon(L::AtSign), move |w, cx| crate::aliases::edit_dialog(d.clone(), w, cx)));
        }
        // leave a folder (a build output, a huge cache) out of the search index
        if single_dir && cx.global::<crate::settings::Settings>().name_index && crate::archive_view::split(&target_dir).is_none() {
            let key = target_dir.to_string_lossy().trim_end_matches('\\').to_string();
            let out = cx.global::<crate::settings::Settings>().index_excluded.iter().any(|e| e.eq_ignore_ascii_case(&key));
            v.push(
                Item::action(t!("Aramada dizine alma"), icon(L::SearchSlash), move |_, cx| {
                    let key = key.clone();
                    crate::settings::update(cx, |s| {
                        if out {
                            s.index_excluded.retain(|e| !e.eq_ignore_ascii_case(&key));
                        } else {
                            s.index_excluded.push(key);
                        }
                    });
                })
                .checked(out),
            );
        }
        if single_dir && show_flatten && !in_archive {
            let d = target_dir.clone();
            v.push(Item::action(t!("Klasörü düzleştir"), icon(L::FolderTree), act(me, move |v, w, cx| v.flatten_folder(d.clone(), w, cx))));
        }
        // type-specific actions (Files: rotate, set as background, install font, run as administrator)
        let type_from = v.len();
        if !items.is_empty() && items.iter().all(|p| rf_win::rotate::can_rotate(p) && crate::archive_view::split(p).is_none()) {
            v.push(Item::action(t!("Sağa döndür"), icon(L::RotateCw), act(me, |v, w, cx| v.rotate_selection(true, w, cx))));
            v.push(Item::action(t!("Sola döndür"), icon(L::RotateCcw), act(me, |v, w, cx| v.rotate_selection(false, w, cx))));
        }
        if items.len() == 1 {
            let p = items[0].clone();
            if rf_win::fileactions::is_image(&p) {
                v.push(Item::action(t!("Masaüstü arka planı yap"), icon(L::Monitor), move |_, _| {
                    let p = p.clone();
                    std::thread::spawn(move || {
                        // A picture inside an archive has no file of its own: Windows was given the path inside the archive, could not
                        // read it and left a plain colour (the owner, 2026-11-18: the background turned purple and pink). It is
                        // extracted first, into the app's own folder (not the temp folder, which is cleaned after two days: the
                        // background must still be there next week).
                        let Some(real) = wallpaper_file(&p) else {
                            log::warn!("wallpaper: the picture could not be extracted from its archive");
                            return;
                        };
                        if let Err(e) = rf_win::fileactions::set_wallpaper(&real) {
                            log::warn!("wallpaper: {e:#}");
                        }
                    });
                }));
            } else if rf_win::fileactions::is_msi(&p) && !in_archive {
                // a Windows Installer package: the verbs Windows gives it (it has no "run as administrator"; the installer asks for it
                // itself), as in Explorer's menu (issue 211)
                for (label, verb, ic) in [(t!("Kur"), "open", L::Package), (t!("Onar"), "repair", L::Wrench), (t!("Programı kaldır"), "uninstall", L::Trash)] {
                    let p = p.clone();
                    v.push(Item::action(label, icon(ic), move |_, _| {
                        let p = p.clone();
                        rf_win::sta::run(move |owner| {
                            if let Err(e) = rf_win::ctxmenu::invoke_verb(owner, &ShellTarget::One(p), verb, cursor()) {
                                log::warn!("msi {verb}: {e:#}");
                            }
                        });
                    }));
                }
            } else if rf_win::fileactions::is_disk_image(&p) && !in_archive && rf_win::fileactions::has_verb(&p, "mount") {
                // a disk image: Windows' own Mount, which was only under "Show more options" (issue 310)
                v.push(Item::action(t!("Bağla"), icon(L::Disc3), move |_, _| {
                    let p = p.clone();
                    rf_win::sta::run(move |owner| {
                        if let Err(e) = rf_win::ctxmenu::invoke_verb(owner, &ShellTarget::One(p), "mount", cursor()) {
                            log::warn!("mount: {e:#}");
                        }
                    });
                }));
            } else if rf_win::fileactions::is_runnable(&p) && !in_archive {
                v.push(Item::action(t!("Yönetici olarak çalıştır"), icon(L::ShieldCheck), move |_, _| {
                    let p = p.clone();
                    rf_win::sta::run(move |owner| {
                        if let Err(e) = rf_win::ctxmenu::invoke_verb(owner, &ShellTarget::One(p), "runas", cursor()) {
                            log::warn!("runas: {e:#}");
                        }
                    });
                }));
            }
        }
        if !items.is_empty() && items.iter().all(|p| rf_win::fileactions::is_font(p)) {
            let fonts = items.to_vec();
            v.push(Item::action(t!("Yazı tipini yükle"), icon(L::Type), move |w, cx| {
                let fonts = fonts.clone();
                let title = format!("{} {}", t!("Yazı tipi yükleniyor:"), crate::i18n::items(fonts.len()));
                crate::status_center::start_with(cx, title, move |_| {
                    for f in &fonts {
                        // (a font inside an archive is installed from a copy extracted first)
                        let real = if f.exists() { Some(f.clone()) } else { crate::archive_view::real_copy(f) };
                        let real = real.ok_or_else(|| anyhow::anyhow!("{}", t!("Arşivden çıkarılamadı")))?;
                        rf_win::fileactions::install_font_for_user(&real)?;
                    }
                    Ok(true)
                }, None);
                let _ = w;
            }));
        }
        raise(&mut v, type_from, &mut type_at);
        v.push(Item::Separator);

        // archives
        let zip_items = items.to_vec();
        let z7_items = items.to_vec();
        let dlg_items = items.to_vec();
        if mp.compress && !in_archive {
            let compress_icon = icon(L::Archive);
            let quick = cx.global::<crate::settings::Settings>().quick_compress;
            let quick_items = items.to_vec();
            let sub_items: Vec<Item> = vec![
                Item::action(t!("Arşiv oluştur…"), icon(L::Archive), move |w, cx| create_archive_dialog(dlg_items.clone(), w, cx)),
                Item::Separator,
                Item::action(t!("ZIP olarak sıkıştır"), icon(L::FileArchive), move |_, cx| compress(cx, zip_items.clone(), rf_archive::Format::Zip)),
                Item::action(t!("7z olarak sıkıştır"), icon(L::FileArchive), move |_, cx| compress(cx, z7_items.clone(), rf_archive::Format::SevenZ)),
            ]
            .into_iter()
            .chain(crate::archive_view::winrar().is_some().then(|| {
                let rar_items = items.to_vec();
                Item::action(t!("RAR olarak sıkıştır"), icon(L::FileArchive), move |_, cx| {
                    if let Some((dir, stem)) = archive_stem(&rar_items) {
                        let dest = rf_win::fileops::unique_name(&dir, &stem, ".rar");
                        crate::status_center::compress_rar(cx, rar_items.clone(), dest, 5, None);
                    }
                })
            }))
            .collect();
            // (a click on the row compresses at once in the format chosen in Settings; hovering shows the other ways)
            v.push(match quick_compress_format(quick) {
                Some(f) => Item::sub_click(t!("Sıkıştır"), compress_icon, sub_items, move |_, cx| quick_compress(cx, quick_items.clone(), f)),
                None => Item::sub(t!("Sıkıştır"), compress_icon, sub_items),
            });
        }
        // a .torrent file, with HyperDownload installed: downloaded next to it
        if items.len() == 1
            && items[0].extension().is_some_and(|e| e.eq_ignore_ascii_case("torrent"))
            && items[0].is_file()
            && crate::addons::installed(crate::hyperdownload::ID).is_some()
        {
            let file = items[0].clone();
            v.push(Item::action(t!("HyperDownload ile indir"), icon(L::Download), move |_, cx| {
                let dir = file.parent().map(|d| d.to_path_buf()).unwrap_or_default();
                crate::hyperdownload::download_torrent(file.clone(), dir, cx)
            }));
        }
        let extract_from = v.len();
        if mp.compress && !items.is_empty() && items.iter().all(|p| rf_archive::is_supported(p)) {
            use crate::status_center::ExtractTo;
            let list = items.to_vec();
            let act = |label: String, ic: L, to: ExtractTo| {
                let list = list.clone();
                Item::action(label, icon(ic), move |_, cx| crate::status_center::extract_to(cx, list.clone(), to.clone()))
            };
            let named = if items.len() > 1 {
                t!("Her birini kendi klasörüne çıkar").to_string()
            } else {
                format!("{} {}\\", t!("Klasöre çıkar:"), crate::ops::archive_stem(&items[0]))
            };
            let pick = list.clone();
            v.push(Item::sub(
                t!("Çıkar"),
                icon(L::FolderOutput),
                vec![
                    act(t!("Buraya çıkar").to_string(), L::FolderDown, ExtractTo::Here),
                    act(t!("Akıllı çıkar").to_string(), L::Zap, ExtractTo::Smart),
                    act(named, L::FolderInput, ExtractTo::Named),
                    Item::action(t!("Şuraya çıkar…"), icon(L::FolderSearch), move |_, cx| crate::status_center::extract_pick(cx, pick.clone())),
                ],
            ));
        }
        raise(&mut v, extract_from, &mut type_at);
        if mp.tags && !in_archive {
            v.push(Item::sub(t!("Etiketler"), icon(L::Tag), tag_items(me, items, cx)));
        }
        if mp.shelf && !in_archive {
            let shelf_items = items.to_vec();
            v.push(Item::action(t!("Rafa ekle"), icon(L::Inbox), act(me, move |_, _, cx| cx.emit(FolderEvent::AddToShelf(shelf_items.clone())))).hint("Ctrl+Shift+S"));
        }
        if !in_archive && crate::ferry::running(cx) {
            v.push(Item::sub(t!("Ferry ile gönder"), icon(L::Send), crate::ferry::menu_items(items, cx)));
        }
        if items.len() > 1 && !self.is_search() && !in_archive {
            let b = items.to_vec();
            v.push(Item::action(t!("Toplu yeniden adlandır…"), icon(L::PencilLine), act(me, move |_, _, cx| cx.emit(FolderEvent::BulkRename(b.clone())))).hint("F2"));
        }
        if self.dual && !in_archive {
            let (c, m) = (items.to_vec(), items.to_vec());
            v.push(Item::action(t!("Diğer panele kopyala"), icon(L::CopyPlus), act(me, move |_, _, cx| cx.emit(FolderEvent::ToOtherPane(Transfer::Copy, c.clone())))).hint("Shift+F5"));
            v.push(Item::action(t!("Diğer panele taşı"), icon(L::MoveRight), act(me, move |_, _, cx| cx.emit(FolderEvent::ToOtherPane(Transfer::Move, m.clone())))).hint("Shift+F6"));
        }
        if single_dir && mp.terminal && !in_archive {
            v.push(Item::Separator);
            let d = target_dir.clone();
            v.push(Item::action(t!("Terminalde aç"), icon(L::SquareTerminal), move |_, _| open_terminal(d.clone())).hint("Ctrl+Shift+`"));
        }
        v.push(Item::Separator);
        v.push(Item::Sub {
            label: t!("Diğer seçenekleri göster").into(),
            icon: icon(L::Ellipsis),
            items: Sub::Pending { key: "more", fallback: None },
            click: None,
        });
        if explorer_theme {
            v.push(Item::action(t!("Özellikler"), icon(L::Wrench), act(me, |v, _, cx| v.properties(cx))).hint("Alt+Enter"));
        }
        let mut quick = self.quick_clipboard(me, true);
        if explorer_theme {
            quick.retain(|q| !matches!(q.icon, L::ClipboardPaste | L::Wrench));
        }
        if in_archive {
            // (Windows' share sheet needs real files)
            quick.retain(|q| q.icon != L::Share2);
        }
        Menu { quick, items: v }
    }

    fn background_menu(&self, me: &Me, dir: &std::path::Path, cx: &mut Context<Self>) -> Menu {
        let mode = self.view_mode;
        let sort = self.sort;
        let group = self.group;
        let mut v: Vec<Item> = Vec::new();
        v.push(Item::sub(t!("Sayfa düzeni"), icon(L::LayoutGrid), layout_items(me, mode, self.grid_style)));
        v.push(Item::sub(t!("Sırala"), icon(L::ArrowUpDown), sort_items(me, sort, self.sort_offers(cx))));
        let mut group_menu = group_items(me, group);
        group_menu.extend(self.property_group_items(me, cx));
        v.push(Item::sub(t!("Gruplandır"), icon(L::ListTree), group_menu));
        v.push(Item::action(t!("Yenile"), icon(L::RotateCw), act(me, |v, _, cx| v.refresh(cx))).hint("Ctrl+R"));
        v.push(Item::Separator);
        // what Ctrl+Z and Ctrl+Y would do, when there is something (issue 55: Explorer's "Undo Delete")
        let (undo, redo) = crate::undo::next_labels();
        if undo.is_some() {
            v.push(Item::action(undo_text(undo), icon(L::Undo2), |_, cx| crate::undo::undo(cx)).hint("Ctrl+Z"));
        }
        if redo.is_some() {
            v.push(Item::action(redo_text(redo), icon(L::Redo2), |_, cx| crate::undo::redo(cx)).hint("Ctrl+Y"));
        }
        let fallback = Rc::new(own_new_items(me));
        v.push(Item::Sub { label: t!("Yeni").into(), icon: icon(L::CirclePlus), items: Sub::Pending { key: "new", fallback: Some(fallback) }, click: None });
        let in_archive = crate::archive_view::split(dir).is_some() && !dir.exists();
        if !in_archive {
            v.push(Item::action(t!("Kısayol yapıştır"), icon(L::ClipboardPaste), act(me, |v, w, cx| v.paste_shortcuts(w, cx))).enabled(rf_win::clipboard::has_files()));
        }
        // HyperDownload installed: a link downloaded into this folder (it was only in the command palette)
        if !in_archive && !rf_win::ftp::is_ftp(dir) && crate::addons::installed(crate::hyperdownload::ID).is_some() {
            v.push(Item::action(t!("Bağlantıdan indir…"), icon(L::Download), act(me, |v, w, cx| v.download_from_link(w, cx))));
        }
        let d = dir.to_path_buf();
        v.push(Item::action(t!("Kenar çubuğuna sabitle"), icon(L::Pin), act(me, move |v, _, cx| v.pin(d.clone(), cx))));
        v.push(Item::action(t!("Yolu kopyala"), icon(L::ClipboardCopy), act(me, |v, _, cx| v.copy_path(cx))));
        if !self.is_search() && !crate::archive_view::is_browsable_path(dir) && !rf_win::ftp::is_ftp(dir) {
            let d = dir.to_path_buf();
            v.push(Item::action(t!("Disk kullanımını göster"), icon(L::ChartPie), act(me, move |_, _, cx| {
                cx.emit(FolderEvent::OpenInNewTab(Location::DiskUsage(d.clone())))
            })));
        }
        let show_hidden = self.filter.show_hidden;
        v.push(Item::action(t!("Gizli öğeleri göster"), icon(L::Eye), act(me, |v, _, cx| v.toggle_hidden(cx))).checked(show_hidden).hint("Ctrl+H"));
        v.push(Item::Separator);
        if !in_archive {
            let d = dir.to_path_buf();
            v.push(Item::action(t!("Terminalde aç"), icon(L::SquareTerminal), move |_, _| open_terminal(d.clone())).hint("Ctrl+Shift+`"));
        }
        // the folder organizer: only where the folder has rules
        if !self.is_search() && crate::organizer::has_rules_for(dir, cx) {
            let d = dir.to_path_buf();
            v.push(Item::action(t!("Klasörü düzenle"), icon(L::WandSparkles), move |w, cx| crate::organizer::organize_folder(d.clone(), w, cx)));
        }
        // Git (Files: initialize / clone from the folder background)
        if !self.is_search() && !crate::archive_view::is_browsable_path(dir) {
            let mut git_items = Vec::new();
            if self.git.is_none() {
                let (m, d) = (me.clone(), dir.to_path_buf());
                git_items.push(Item::action(t!("Git deposu başlat"), icon(L::GitBranchPlus), move |_, cx| crate::git_ui::init_repo(m.clone(), d.clone(), cx)));
            }
            let (m, d) = (me.clone(), dir.to_path_buf());
            git_items.push(Item::action(t!("Depoyu klonla…"), icon(L::GitFork), move |w, cx| crate::git_ui::clone_dialog(m.clone(), d.clone(), w, cx)));
            v.push(Item::sub("Git", icon(L::GitBranch), git_items));
        }
        v.push(Item::Separator);
        v.push(Item::Sub { label: t!("Diğer seçenekleri göster").into(), icon: icon(L::Ellipsis), items: Sub::Pending { key: "more", fallback: None }, click: None });
        Menu { quick: self.quick_clipboard(me, false), items: v }
    }

    /// F2 from the menu: bulk rename for several items, inline for one.
    pub(crate) fn rename_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.selection_paths().len() > 1 {
            cx.emit(FolderEvent::BulkRename(self.selection_paths()));
        } else {
            self.begin_rename(window, cx);
        }
    }

    /// Windows' share sheet for the selection, over this window (`rf_win::share`: the context menu's verb failed, issue 48).
    pub(crate) fn share(&self, window: &Window, cx: &mut App) {
        crate::share::open(crate::look::window::hwnd_of(window), self.selection_paths(), cx);
    }

    /// Properties of the current folder (background menu).
    pub(crate) fn folder_properties(&self, cx: &mut App) {
        let Some(dir) = self.current_dir() else { return };
        crate::properties_view::open(vec![dir], self.images.clone(), cx);
    }
}

/// "Geri al: Silme", or plain "Geri al" when there is nothing to undo.
/// Moves the rows added since `from` up to row `to`, and `to` past them.
///
/// WHY (issue 310): the commands of the file's own type (Install, Mount, Extract, Run as administrator) are what a right click on
/// such a file is usually for, and they sat in the second and third blocks, below Copy path and Compress.
fn raise(v: &mut Vec<Item>, from: usize, to: &mut usize) {
    let moved: Vec<Item> = v.drain(from..).collect();
    let n = moved.len();
    v.splice(*to..*to, moved);
    *to += n;
}

fn undo_text(what: Option<&str>) -> String {
    match what {
        Some(w) => format!("{}: {w}", t!("Geri al")),
        None => t!("Geri al").to_string(),
    }
}

fn redo_text(what: Option<&str>) -> String {
    match what {
        Some(w) => format!("{}: {w}", t!("Yinele")),
        None => t!("Yinele").to_string(),
    }
}

fn layout_items(me: &Me, mode: ViewMode, style: crate::folder_view::GridStyle) -> Vec<Item> {
    use crate::folder_view::GridStyle;
    let layout = |label: &str, l: L, m: ViewMode, st: GridStyle, hint: &str| {
        let on = mode == m && (m != ViewMode::Grid || style == st);
        Item::action(label.to_string(), icon(l), act(me, move |v, _, cx| v.set_layout(m, st, cx))).checked(on).hint(hint.to_string())
    };
    vec![
        layout(t!("Ayrıntılar"), L::List, ViewMode::Details, GridStyle::Tiles, "Ctrl+Shift+1"),
        layout(t!("Liste"), L::TextAlignJustify, ViewMode::Grid, GridStyle::List, "Ctrl+Shift+5"),
        layout(t!("Kartlar"), L::LayoutList, ViewMode::Grid, GridStyle::Cards, "Ctrl+Shift+4"),
        layout(t!("Izgara"), L::LayoutGrid, ViewMode::Grid, GridStyle::Tiles, "Ctrl+Shift+2"),
        layout(t!("Sütunlar"), L::Columns3, ViewMode::Columns, GridStyle::Tiles, "Ctrl+Shift+3"),
    ]
}

fn sort_items(me: &Me, sort: rf_core::SortSpec, offers: crate::folder_view::sort_ext::SortOffers) -> Vec<Item> {
    let by = |label: &str, k: SortKey| {
        Item::action(label.to_string(), MenuIcon::None, act(me, move |v, _, cx| {
            let mut s = v.sort;
            s.key = k;
            v.set_sort_spec(s, cx)
        }))
        .checked(sort.key == k)
    };
    let dir_item = |label: &str, d: SortDir| {
        Item::action(label.to_string(), MenuIcon::None, act(me, move |v, _, cx| {
            let mut s = v.sort;
            s.dir = d;
            v.set_sort_spec(s, cx)
        }))
        .checked(sort.dir == d)
    };
    let mut v = vec![
        by(t!("Ad"), SortKey::Name),
        by(t!("Değiştirme tarihi"), SortKey::Modified),
        by(t!("Oluşturma tarihi"), SortKey::Created),
        by(t!("Tür"), SortKey::Type),
        by(t!("Boyut"), SortKey::Size),
    ];
    // the keys that need more than the entry: tag, the folder (results from several folders), sync state
    if offers.tag {
        v.push(by(t!("Etiket"), SortKey::Tag));
    }
    if offers.path {
        v.push(by(t!("Konum"), SortKey::Path));
    }
    if offers.sync {
        v.push(by(t!("Eşitleme durumu"), SortKey::Sync));
    }
    v.push(Item::Separator);
    v.push(dir_item(t!("Artan"), SortDir::Asc));
    v.push(dir_item(t!("Azalan"), SortDir::Desc));
    v
}

/// Folders first / files first / mixed (Files' three radio items under Sort and Group).
fn order_items(me: &Me, sort: rf_core::SortSpec) -> Vec<Item> {
    let order = |label: &str, dirs: bool, files: bool| {
        Item::action(label.to_string(), MenuIcon::None, act(me, move |v, _, cx| {
            let mut s = v.sort;
            s.dirs_first = dirs;
            s.files_first = files;
            v.set_sort_spec(s, cx)
        }))
        .checked(sort.dirs_first == dirs && (dirs || sort.files_first == files))
    };
    vec![
        order(t!("Önce klasörleri sırala"), true, false),
        order(t!("Önce dosyaları sırala"), false, true),
        order(t!("Dosyaları ve klasörleri birlikte sırala"), false, false),
    ]
}

fn group_items(me: &Me, group: GroupBy) -> Vec<Item> {
    let grp = |label: &str, g: GroupBy| Item::action(label.to_string(), MenuIcon::None, act(me, move |v, _, cx| v.set_group(g, cx))).checked(group == g);
    vec![
        grp(t!("Yok"), GroupBy::None),
        grp(t!("Ad"), GroupBy::Name),
        grp(t!("Tür"), GroupBy::Type),
        grp(t!("Değiştirme tarihi"), GroupBy::Modified),
        grp(t!("Yıl"), GroupBy::Year),
        grp(t!("Ay"), GroupBy::Month),
        grp(t!("Gün"), GroupBy::Day),
        grp(t!("Oluşturma tarihi"), GroupBy::Created),
        grp(t!("Boyut"), GroupBy::Size),
        grp(t!("Etiket"), GroupBy::Tag),
        grp(t!("Klasör"), GroupBy::Folder),
    ]
}

/// Drop-down menus of the command bar.
pub enum BarMenu {
    New,
    Sort,
    /// The "…" button: undo and redo, the selection commands, the folder's properties, Settings (issue 55: as Explorer's "See more"
    /// menu has them).
    More,
    /// Layout + view toggles; the flags are workspace state shown as check marks.
    View { dual: bool, shelf: bool },
}

impl FolderView {
    pub fn show_bar_menu(&mut self, which: BarMenu, pos: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let me = cx.entity().downgrade();
        let explorer_theme = cx.global::<crate::settings::Settings>().windows_file_explorer_theme;
        let new_menu = matches!(&which, BarMenu::New);
        let sort_menu = matches!(&which, BarMenu::Sort);
        let layout_menu = matches!(&which, BarMenu::View { .. });
        let original_icons = explorer_theme && (new_menu || sort_menu || layout_menu);
        let context_style = matches!(&which, BarMenu::More) || original_icons;
        let items = match which {
            BarMenu::New => {
                let Some(dir) = self.current_dir() else { return };
                let menu = Menu { quick: Vec::new(), items: own_new_items(&me) };
                let view = if original_icons {
                    fluent_menu::show_context_original_icons(menu, pos, window, cx)
                } else {
                    fluent_menu::show_context(menu, pos, window, cx)
                };
                // the shell's templates (Word, Excel…) join as soon as the shell answers
                let (tx, rx) = oneshot::channel();
                rf_win::sta::run(move |owner| {
                    let _ = tx.send(rf_win::ctxmenu::build(owner, &ShellTarget::Background(dir), false).ok());
                });
                let weak = view.downgrade();
                cx.spawn(async move |_, cx| {
                    let Ok(Some((session, shell))) = rx.await else { return };
                    let _ = cx.update(|cx| {
                        if let (_, _, Pending::Ready(items)) = split_shell_items(&me, session, shell, true, &[], cx) {
                            if let Some(m) = weak.upgrade() {
                                m.update(cx, |m, cx| m.set_root((*items).clone(), cx));
                            }
                        }
                    });
                })
                .detach();
                return;
            }
            BarMenu::More => {
                let (undo, redo) = crate::undo::next_labels();
                let none = self.selected.is_empty();
                vec![
                    Item::action(
                        undo_text(undo),
                        icon(L::Undo2),
                        move |_, cx| crate::undo::undo(cx),
                    )
                    .hint("Ctrl+Z")
                    .enabled(undo.is_some()),
                    Item::action(
                        redo_text(redo),
                        icon(L::Redo2),
                        move |_, cx| crate::undo::redo(cx),
                    )
                    .hint("Ctrl+Y")
                    .enabled(redo.is_some()),
                    Item::Separator,
                    Item::action(
                        t!("Tümünü seç"),
                        icon(L::SquareCheck),
                        act(&me, |v, _, cx| v.select_all_items(cx)),
                    )
                    .hint("Ctrl+A"),
                    Item::action(
                        t!("Hiçbirini seçme"),
                        icon(L::Square),
                        act(&me, |v, _, cx| v.select_no_items(cx)),
                    )
                    .enabled(!none),
                    Item::action(
                        t!("Seçimi tersine çevir"),
                        icon(L::ArrowLeftRight),
                        act(&me, |v, _, cx| v.invert_selected_items(cx)),
                    ),
                    Item::Separator,
                    Item::action(
                        t!("Özellikler"),
                        icon(L::Wrench),
                        act(&me, |v, _, cx| v.folder_properties(cx)),
                    ),
                    Item::action(
                        t!("Ayarlar"),
                        icon(L::Settings),
                        |w, cx| w.dispatch_action(Box::new(crate::actions::GoSettings), cx),
                    )
                    .hint("Ctrl+,"),
                    // (closing the window leaves rustyfiles running with background mode or the Dock: issue 410)
                    Item::action(t!("Çıkış"), icon(L::LogOut), |_, cx| cx.defer(crate::background::quit_all)),
                ]
            }
            BarMenu::Sort => {
                // Files' layout: Sort ▸ / Group ▸, then the folder/file order; these apply to
                // this folder only (each folder remembers its own) unless "sync" is on
                let mut v = vec![
                    Item::sub(t!("Sırala"), icon(L::ArrowUpDown), sort_items(&me, self.sort, self.sort_offers(cx))),
                    Item::sub(t!("Gruplandır"), icon(L::ListTree), {
                        // (the chosen property columns follow the fixed keys; none chosen: nothing is added)
                        let mut g = group_items(&me, self.group);
                        g.extend(self.property_group_items(&me, cx));
                        g
                    }),
                    Item::Separator,
                ];
                v.extend(order_items(&me, self.sort));
                if !cx.global::<crate::settings::Settings>().sync_layout && matches!(self.location(), Location::Path(_)) {
                    v.push(Item::Separator);
                    v.push(Item::action(t!("Tüm klasörler için varsayılan yap"), icon(L::CopyCheck), act(&me, |v, _, cx| v.make_layout_default(cx))));
                }
                v
            }
            BarMenu::View { dual, shelf } if std::env::var_os("RF_INWINDOW_MENU").is_none() => {
                // Files' layout flyout (tiles, size slider, switches)
                // opened after this update: its first frame reads this view
                window.defer(cx, move |window, cx| crate::layout_panel::open(me, dual, shelf, pos, window, cx));
                return;
            }
            BarMenu::View { dual, shelf } => {
                let mut v = layout_items(&me, self.view_mode, self.grid_style);
                v.push(Item::Separator);
                v.push(
                    Item::action(t!("Gizli öğeleri göster"), icon(L::Eye), act(&me, |v, _, cx| v.toggle_hidden(cx)))
                        .checked(self.filter.show_hidden)
                        .hint("Ctrl+H"),
                );
                v.push(
                    Item::action(t!("Çift panel"), icon(L::Columns2), |w, cx| w.dispatch_action(Box::new(crate::actions::ToggleDualPane), cx))
                        .checked(dual)
                        .hint("Ctrl+Shift+D"),
                );
                v.push(Item::action(t!("Raf"), icon(L::Inbox), |w, cx| w.dispatch_action(Box::new(crate::actions::ToggleShelf), cx)).checked(shelf));
                v
            }
        };
        if context_style {
            let menu = Menu { quick: Vec::new(), items };
            if original_icons {
                fluent_menu::show_context_original_icons(menu, pos, window, cx);
            } else {
                fluent_menu::show_context(menu, pos, window, cx);
            }
        } else {
            fluent_menu::show(Menu { quick: Vec::new(), items }, pos, window, cx);
        }
    }
}

fn own_new_items(me: &Me) -> Vec<Item> {
    vec![own_new_folder(me), own_new_text(me)]
}

fn own_new_folder(me: &Me) -> Item {
    Item::action(t!("Klasör"), MenuIcon::Tinted(L::Folder, crate::theme::kind_color(crate::icons::Category::Folder, true)), act(me, |v, _, cx| v.create_new(true, cx)))
        .hint("Ctrl+Shift+N")
}

fn own_new_text(me: &Me) -> Item {
    Item::action(t!("Metin belgesi"), icon(L::FileText), act(me, |v, _, cx| v.create_new(false, cx)))
}

/// The order of the New menu: Folder and Shortcut first, a line, then every kind of file by its name, A to Z, ours (Text document)
/// among them. The shell lists its templates by their extension (.bmp, .cdr ... .txt, .zip), which reads as no order at all (issue
/// 369). `labels` are the shell templates' names; the answer is where each goes, with `None` for Text document's place.
fn new_menu_order(labels: &[&str], text_label: &str) -> Vec<Option<usize>> {
    let mut order: Vec<Option<usize>> = (0..labels.len()).map(Some).chain(std::iter::once(None)).collect();
    let name = |o: &Option<usize>| o.map_or(text_label, |i| labels[i]);
    order.sort_by(|a, b| rf_core::sort::natural_cmp(&name(a).to_lowercase(), &name(b).to_lowercase()));
    order
}

fn tag_items(me: &Me, items: &[PathBuf], cx: &App) -> Vec<Item> {
    let store = &cx.global::<crate::tags::Tags>().0;
    store
        .defs
        .iter()
        .map(|d| {
            let all = items.iter().all(|p| store.has(p, d.id));
            let (paths, id) = (items.to_vec(), d.id);
            let color = crate::tags::color(d.color);
            Item::action(crate::i18n::td(&d.name), MenuIcon::Dot(color), act(me, move |_, _, cx| {
                crate::tags::update(cx, |t| t.toggle(&paths, id));
                cx.notify();
            }))
            .checked(all)
        })
        .collect()
}

/// Default archive name for `items`: the item's name (one) or the folder's name (several).
fn archive_stem(items: &[PathBuf]) -> Option<(PathBuf, String)> {
    let first = items.first()?;
    let dir = first.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let stem = if items.len() == 1 {
        if first.is_dir() {
            first.file_name().map(|s| s.to_string_lossy().into_owned())
        } else {
            first.file_stem().map(|s| s.to_string_lossy().into_owned())
        }
        .unwrap_or_else(|| "Arşiv".into())
    } else {
        rf_core::history::display_name(&dir)
    };
    Some((dir, stem))
}

/// What a click on "Sıkıştır" makes: `None` when it only opens the submenu (or RAR is chosen and WinRAR is gone).
fn quick_compress_format(q: crate::settings::QuickCompress) -> Option<Option<rf_archive::Format>> {
    use crate::settings::QuickCompress as Q;
    match q {
        Q::Menu => None,
        Q::Zip => Some(Some(rf_archive::Format::Zip)),
        Q::SevenZ => Some(Some(rf_archive::Format::SevenZ)),
        Q::TarZst => Some(Some(rf_archive::Format::TarZst)),
        Q::TarGz => Some(Some(rf_archive::Format::TarGz)),
        Q::Tar => Some(Some(rf_archive::Format::Tar)),
        // (None inside: RAR, through WinRAR)
        Q::Rar => crate::archive_view::winrar().is_some().then_some(None),
    }
}

fn quick_compress(cx: &mut App, items: Vec<PathBuf>, format: Option<rf_archive::Format>) {
    match format {
        Some(f) => compress(cx, items, f),
        None => {
            if let Some((dir, stem)) = archive_stem(&items) {
                let dest = rf_win::fileops::unique_name(&dir, &stem, ".rar");
                crate::status_center::compress_rar(cx, items, dest, 5, None);
            }
        }
    }
}

fn compress(cx: &mut App, items: Vec<PathBuf>, format: rf_archive::Format) {
    let Some((dir, stem)) = archive_stem(&items) else { return };
    let dest = dir.join(format!("{stem}.{}", format.extension()));
    crate::status_center::compress(cx, items, dest, format);
}

/// Files' "Create archive" dialog: name, format and compression level.
fn create_archive_dialog(items: Vec<PathBuf>, window: &mut Window, cx: &mut App) {
    use crate::prompt::{Button, Choice, Kind};
    let Some((dir, stem)) = archive_stem(&items) else { return };
    let count = items.len();
    let format = std::rc::Rc::new(std::cell::Cell::new(0usize));
    let level = std::rc::Rc::new(std::cell::Cell::new(2usize));
    let protect = std::rc::Rc::new(std::cell::Cell::new(0usize));
    let split = std::rc::Rc::new(std::cell::Cell::new(0usize));
    let dict = std::rc::Rc::new(std::cell::Cell::new(0usize));
    // (None = RAR, WinRAR's format: offered only when its Rar.exe is installed)
    let mut formats = vec![
        ("ZIP", Some(rf_archive::Format::Zip)),
        ("7z", Some(rf_archive::Format::SevenZ)),
        ("tar.zst", Some(rf_archive::Format::TarZst)),
        ("tar.gz", Some(rf_archive::Format::TarGz)),
        ("tar", Some(rf_archive::Format::Tar)),
    ];
    if crate::archive_view::winrar().is_some() {
        formats.push(("RAR", None));
    }
    let choices = vec![
        Choice { label: t!("Biçim").into(), options: formats.iter().map(|(n, _)| SharedString::from(*n)).collect(), selected: format.clone() },
        Choice {
            label: t!("Sıkıştırma düzeyi").into(),
            options: vec![t!("Yok").into(), t!("Hızlı").into(), t!("Normal").into(), t!("En yüksek").into()],
            selected: level.clone(),
        },
        // ZIP: AES-256; 7z: AES-256 with encrypted names
        Choice { label: t!("Parola").into(), options: vec![t!("Yok").into(), t!("Parola koy…").into()], selected: protect.clone() },
        // (parts as 7-Zip names them: name.zip.001, .002, ...)
        Choice {
            label: t!("Parçalara böl").into(),
            options: std::iter::once(SharedString::from(t!("Yok"))).chain(SPLIT_SIZES.iter().map(|(n, _)| SharedString::from(*n))).collect(),
            selected: split.clone(),
        },
        Choice {
            label: t!("Sözlük boyutu (7z)").into(),
            options: std::iter::once(SharedString::from(t!("Otomatik"))).chain(DICT_SIZES.iter().map(|(n, _)| SharedString::from(*n))).collect(),
            selected: dict.clone(),
        },
    ];
    let create = Button::new(t!("Oluştur"), Kind::Primary, move |name, window, cx| {
        let name = name.unwrap_or_default();
        let name = name.trim();
        if name.is_empty() {
            return Err(t!("Bir ad girin").into());
        }
        if name.contains(['\\', '/', ':', '*', '?', '"', '<', '>', '|']) {
            return Err(t!("Ad şu karakterleri içeremez: \\ / : * ? \" < > |").into());
        }
        let lvl = [0, 1, 5, 9][level.get().min(3)];
        let fmt = formats.get(format.get()).and_then(|f| f.1);
        let volume_size = split.get().checked_sub(1).and_then(|i| SPLIT_SIZES.get(i)).map(|s| s.1);
        let dict_size = dict.get().checked_sub(1).and_then(|i| DICT_SIZES.get(i)).map(|s| s.1);
        let Some(fmt) = fmt else {
            if volume_size.is_some() {
                return Err(t!("RAR parçalara bölünemiyor; ZIP ya da 7z seçin").into());
            }
            let dest = dir.join(format!("{name}.rar"));
            let items = items.clone();
            if protect.get() == 1 {
                let finish: Finish = Rc::new(move |cx, pw| crate::status_center::compress_rar(cx, items.clone(), dest.clone(), lvl, Some(pw)));
                let d = dir.join(format!("{name}.rar"));
                window.defer(cx, move |window, cx| archive_password(d, finish, None, window, cx));
            } else {
                crate::status_center::compress_rar(cx, items, dest, lvl, None);
            }
            return Ok(());
        };
        let dest = dir.join(format!("{name}.{}", fmt.extension()));
        let opts = rf_archive::CompressOptions { password: None, dict_size, volume_size };
        if protect.get() == 1 {
            if !fmt.encrypts() {
                return Err(t!("tar biçimleri parola desteklemez; ZIP ya da 7z seçin").into());
            }
            // asked after this dialog closes (one dialog at a time)
            let items = items.clone();
            let d = dest.clone();
            let finish: Finish = Rc::new(move |cx, pw| {
                let opts = rf_archive::CompressOptions { password: Some(pw), ..opts.clone() };
                crate::status_center::compress_opts(cx, items.clone(), dest.clone(), fmt, lvl, opts)
            });
            window.defer(cx, move |window, cx| archive_password(d, finish, None, window, cx));
            return Ok(());
        }
        crate::status_center::compress_opts(cx, items.clone(), dest, fmt, lvl, opts);
        Ok(())
    });
    crate::prompt::show_with(
        t!("Arşiv oluştur"),
        Some(format!("{} — {}", crate::i18n::items(count),t!("uzantı biçime göre eklenir"))),
        Some((stem, t!("Arşiv adı").to_string())),
        choices,
        vec![create, Button::cancel()],
        window,
        cx,
    );
}

/// "Split into parts" sizes: e-mail attachments, a CD, FAT32's file limit (4 GB less 1 MB).
const SPLIT_SIZES: &[(&str, u64)] = &[("25 MB", 25 << 20), ("100 MB", 100 << 20), ("700 MB", 700 << 20), ("1 GB", 1 << 30), ("4 GB", (4 << 30) - (1 << 20))];
const DICT_SIZES: &[(&str, u64)] = &[("1 MB", 1 << 20), ("16 MB", 16 << 20), ("64 MB", 64 << 20), ("256 MB", 256 << 20)];

type Finish = Rc<dyn Fn(&mut App, String)>;

/// "Create archive" with a password: typed twice (`first` = the first entry), then `finish`.
fn archive_password(dest: PathBuf, finish: Finish, first: Option<String>, window: &mut Window, cx: &mut App) {
    use crate::prompt::{Button, Kind};
    let again = first.is_some();
    let title = if again { t!("Parolayı tekrar girin") } else { t!("Arşiv parolası") };
    let msg = format!("{} — {}", rf_core::history::display_name(&dest), t!("Parolayı unutursanız arşiv açılamaz."));
    let label = if again { t!("Oluştur") } else { t!("Devam") };
    crate::prompt::show_secret(
        title,
        Some(msg),
        t!("Parola").to_string(),
        vec![
            Button::new(label, Kind::Primary, move |text, window, cx| {
                let pw = text.unwrap_or_default();
                if pw.is_empty() {
                    return Err(t!("Parola boş olamaz").into());
                }
                match &first {
                    None => {
                        let (d, f) = (dest.clone(), finish.clone());
                        window.defer(cx, move |window, cx| archive_password(d, f, Some(pw), window, cx));
                    }
                    Some(p) if *p == pw => finish(cx, pw),
                    Some(_) => return Err(t!("Parolalar eşleşmiyor").into()),
                }
                Ok(())
            }),
            Button::cancel(),
        ],
        window,
        cx,
    );
}

fn open_terminal(d: PathBuf) {
    std::thread::spawn(move || {
        let _ = rf_win::shell::open_terminal(&d);
    });
}

/// The kind of thing a menu is for, as far as "Show more options" goes: the folder's background, or an item by its extension (the
/// shell's commands for an item depend on its type; a folder and a file without one are one kind here, as the disk is not asked on
/// the interface's thread). Pure.
fn more_kind(background: bool, items: &[PathBuf]) -> String {
    if background {
        return "<background>".into();
    }
    match items {
        [one] => one.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_else(|| "<none>".into()),
        _ => "<several>".into(),
    }
}

static MORE_EMPTY: std::sync::LazyLock<parking_lot::Mutex<std::collections::HashMap<String, bool>>> =
    std::sync::LazyLock::new(|| parking_lot::Mutex::new(std::collections::HashMap::new()));

/// Whether "Show more options" came back empty the last time for this kind of item.
fn more_was_empty(kind: &str) -> bool {
    MORE_EMPTY.lock().get(kind).copied().unwrap_or(false)
}

fn remember_more(kind: String, empty: bool) {
    MORE_EMPTY.lock().insert(kind, empty);
}

/// "<name> - Kısayol.lnk" next to each item (unique name), like Explorer's "Create shortcut".
fn create_shortcuts(items: Vec<PathBuf>) {
    std::thread::spawn(move || {
        rf_win::shell::ensure_com();
        let mut made = Vec::new();
        for target in items {
            let (Some(parent), Some(name)) = (target.parent(), target.file_name()) else { continue };
            let base = format!("{} - {}", name.to_string_lossy(), t!("Kısayol"));
            let mut link = parent.join(format!("{base}.lnk"));
            let mut n = 2;
            while link.exists() {
                link = parent.join(format!("{base} ({n}).lnk"));
                n += 1;
            }
            match rf_win::shortcut::create(&link, &target) {
                Ok(()) => made.push(link),
                Err(e) => log::warn!("shortcut: {e:#}"),
            }
        }
        // (Ctrl+Z takes them away, and does not undo the operation before them: see `paste_shortcuts`)
        if !made.is_empty() {
            crate::undo::push(crate::undo::UndoOp::CreatedAll { paths: made });
        }
    });
}

/// One of the user's own programs for a kind of file, and its icon when the shell gave one.
type OwnProgram = (String, Option<rf_win::thumbs::Bgra>);

/// A packaged app's command for the menu, with its icon read on the worker.
struct Packaged {
    id: usize,
    title: String,
    icon: Option<rf_win::thumbs::Bgra>,
    enabled: bool,
    sub: Vec<Packaged>,
}

/// The packaged commands with their icons (on the shell STA: the icons are files' own icons).
fn packaged_commands(shown: Vec<rf_win::pkgverbs::Shown>) -> Vec<Packaged> {
    shown
        .into_iter()
        .map(|s| Packaged {
            id: s.id,
            // (the menu marks no access keys: "&Open in Terminal" is "Open in Terminal")
            title: s.title.replace("&&", "\u{1}").replace('&', "").replace('\u{1}', "&"),
            icon: s.icon.and_then(|p| rf_win::thumbs::get_image(&p, 32, rf_win::thumbs::Kind::Icon).ok()),
            enabled: s.enabled,
            sub: packaged_commands(s.sub),
        })
        .collect()
}

fn packaged_items(cmds: Vec<Packaged>) -> Vec<Item> {
    cmds.into_iter()
        .map(|c| {
            let picture = c.icon.as_ref().and_then(crate::image_cache::to_render_image).map(MenuIcon::Image).unwrap_or_else(|| icon(L::Package));
            if c.sub.is_empty() {
                let id = c.id;
                Item::action(c.title, picture, move |_, _| {
                    rf_win::sta::run(move |_| {
                        if let Err(e) = rf_win::pkgverbs::invoke(id) {
                            log::warn!("packaged menu command: {e:#}");
                        }
                    })
                })
                .enabled(c.enabled)
            } else {
                Item::sub(c.title, picture, packaged_items(c.sub))
            }
        })
        .collect()
}

/// "Show more options" with the packaged apps' commands first, as Windows 11's own menu shows them (issue 107: "Edit with
/// Notepad++", NanaZip, Terminal, ...), then the classic menu's items. They are there also when the classic part failed.
///
/// A program may register the same command in both menus (NanaZip: "NanaZip" twice, one under the other). It is listed once: the new
/// menu's entry unless `prefer_classic` (the setting), then the classic one.
fn with_packaged(packaged: Vec<Packaged>, rest: Pending, prefer_classic: bool) -> Pending {
    if packaged.is_empty() {
        return rest;
    }
    let mut items = packaged_items(packaged);
    let mut classic: Vec<Item> = match &rest {
        Pending::Ready(r) => r.to_vec(),
        _ => Vec::new(),
    };
    let (keep_new, keep_classic) = twins(&items.iter().map(item_title).collect::<Vec<_>>(), &classic.iter().map(item_title).collect::<Vec<_>>(), prefer_classic);
    let mut k = keep_new.into_iter();
    items.retain(|_| k.next().unwrap_or(true));
    let mut k = keep_classic.into_iter();
    classic.retain(|_| k.next().unwrap_or(true));
    let classic = tidy_separators(classic);
    if !classic.is_empty() {
        if !items.is_empty() {
            items.push(Item::Separator);
        }
        items.extend(classic);
    }
    Pending::Ready(Rc::new(items))
}

/// A row's name, `None` for a separator.
fn item_title(it: &Item) -> Option<String> {
    match it {
        Item::Action { label, .. } | Item::Sub { label, .. } => Some(label.to_string()),
        Item::Separator => None,
    }
}

/// Which rows of the new menu's commands and of the classic menu's stay when both name the same command (compared without access
/// keys, case, and a trailing ellipsis): one of each pair goes, the classic one unless `prefer_classic`. Rows without a twin and
/// separators always stay.
fn twins(new: &[Option<String>], classic: &[Option<String>], prefer_classic: bool) -> (Vec<bool>, Vec<bool>) {
    let plain = |s: &String| s.replace('&', "").trim().trim_end_matches("...").trim_end_matches('…').trim().to_lowercase();
    let n: Vec<Option<String>> = new.iter().map(|t| t.as_ref().map(plain).filter(|t| !t.is_empty())).collect();
    let c: Vec<Option<String>> = classic.iter().map(|t| t.as_ref().map(plain).filter(|t| !t.is_empty())).collect();
    let in_c = |t: &Option<String>| t.is_some() && c.contains(t);
    let in_n = |t: &Option<String>| t.is_some() && n.contains(t);
    (n.iter().map(|t| !(prefer_classic && in_c(t))).collect(), c.iter().map(|t| prefer_classic || !in_n(t)).collect())
}

/// No separator first, last, or after another one (rows were taken out between them).
fn tidy_separators(items: Vec<Item>) -> Vec<Item> {
    let mut out: Vec<Item> = Vec::with_capacity(items.len());
    for it in items {
        if matches!(it, Item::Separator) && out.last().is_none_or(|l| matches!(l, Item::Separator)) {
            continue;
        }
        out.push(it);
    }
    if matches!(out.last(), Some(Item::Separator)) {
        out.pop();
    }
    out
}

#[cfg(test)]
mod twin_tests {
    use super::twins;

    fn t(names: &[&str]) -> Vec<Option<String>> {
        names.iter().map(|n| (!n.is_empty()).then(|| n.to_string())).collect()
    }

    #[test]
    fn a_command_in_both_menus_is_listed_once() {
        let new = t(&["NanaZip", "Edit with Notepad++"]);
        let classic = t(&["&NanaZip", "", "Git Sync...", "TortoiseGit"]);
        // by default the new menu's entry stays, as in Windows' own first menu
        assert_eq!(twins(&new, &classic, false), (vec![true, true], vec![false, true, true, true]));
        // the setting keeps the classic one
        assert_eq!(twins(&new, &classic, true), (vec![false, true], vec![true, true, true, true]));
        // names that only look alike are two commands
        assert_eq!(twins(&t(&["Open"]), &t(&["Open with Code"]), false), (vec![true], vec![true]));
        // an ellipsis and the case are not a difference; separators never count as twins
        assert_eq!(twins(&t(&["Share…", ""]), &t(&["share...", ""]), false), (vec![true, true], vec![false, true]));
    }
}

/// An entry of the SendTo folder with its icon (read on the shell's worker thread).
type SendTarget = (rf_win::sendto::Target, Option<rf_win::thumbs::Bgra>);

/// The menu has "Send to" for these items: the setting is on and they are files of one folder on disk (not members of an archive, not
/// on an FTP server: a drop needs the shell's own items of real files).
fn send_to_fits(items: &[PathBuf], cx: &App) -> bool {
    let Some(parent) = items.first().and_then(|p| p.parent()) else { return false };
    cx.global::<crate::settings::Settings>().menu.send_to
        && !rf_win::ftp::is_ftp(parent)
        && items.iter().all(|p| p.parent() == Some(parent) && !(crate::archive_view::split(p).is_some() && !p.exists()))
}

/// The "Send to" submenu: the SendTo folder's entries, then the removable and network drives, as Explorer's menu has them. A click
/// drops the items on the entry (a copy or a link, never a move), on the shell's worker thread.
fn send_to_items(targets: Vec<SendTarget>, drives: Vec<(PathBuf, String, L)>, items: Vec<PathBuf>) -> Pending {
    let send = |target: PathBuf, items: Vec<PathBuf>| {
        move |_: &mut Window, _: &mut App| {
            let (target, items) = (target.clone(), items.clone());
            rf_win::sta::run(move |owner| {
                if let Err(e) = rf_win::sendto::send(owner, &target, &items) {
                    log::warn!("send to {}: {e:#}", target.display());
                }
            });
        }
    };
    let mut out: Vec<Item> = targets
        .into_iter()
        .map(|(t, picture)| {
            let picture = picture.as_ref().and_then(crate::image_cache::to_render_image).map(MenuIcon::Image).unwrap_or_else(|| icon(L::Send));
            Item::action(t.name, picture, send(t.path, items.clone()))
        })
        .collect();
    if !drives.is_empty() {
        if !out.is_empty() {
            out.push(Item::Separator);
        }
        out.extend(drives.into_iter().map(|(root, name, l)| Item::action(name, icon(l), send(root, items.clone()))));
    }
    if out.is_empty() {
        out.push(Item::action(t!("Boş"), icon(L::Send), |_, _| {}).enabled(false));
    }
    Pending::Ready(Rc::new(out))
}

/// "Open with", with the user's own programs for the file's type first (in their order), then what Windows offers. Without a list
/// for the type the submenu is Windows' alone. The programs are there also when the shell's part failed.
fn with_own_programs(me: &Me, own: Vec<OwnProgram>, rest: Pending) -> Pending {
    if own.is_empty() {
        return rest;
    }
    let mut items: Vec<Item> = own
        .into_iter()
        .map(|(program, picture)| {
            let picture = picture.as_ref().and_then(crate::image_cache::to_render_image).map(MenuIcon::Image).unwrap_or_else(|| icon(L::AppWindow));
            let label = rf_core::openrules::program_label(&program);
            Item::action(label, picture, act(me, move |v, window, cx| v.open_selection_in(program.clone(), window, cx)))
        })
        .collect();
    if let Pending::Ready(more) = &rest
        && !more.is_empty()
    {
        items.push(Item::Separator);
        items.extend(more.iter().cloned());
    }
    Pending::Ready(Rc::new(items))
}

/// Turns the shell menu into our three lazy lists: "Show more options" (what is left after
/// hiding our own commands), "Open with" and "New".
/// Whether `it` is the shell's own "Send to" submenu: its entries are the SendTo folder's, `ours` (the menu's own "Send to" lists
/// them already; issue 273: it was there twice). Told by the entries, as the submenu has no verb and its name is Windows' language.
fn is_shells_send_to(it: &ShellItem, ours: &[String]) -> bool {
    if ours.is_empty() || it.children.is_empty() || !it.verb.is_empty() {
        return false;
    }
    let plain = |s: &str| s.replace('&', "").trim().to_lowercase();
    let names: Vec<String> = ours.iter().map(|n| plain(n)).collect();
    let hits = it.children.iter().filter(|c| !c.separator && names.contains(&plain(&c.label))).count();
    hits >= 2.min(names.len()) && hits * 2 >= names.len().min(it.children.iter().filter(|c| !c.separator).count())
}

#[cfg(test)]
mod send_to_tests {
    use super::{ShellItem, is_shells_send_to};

    fn item(label: &str, verb: &str, children: Vec<ShellItem>) -> ShellItem {
        ShellItem { id: 0, label: label.into(), hint: None, verb: verb.into(), icon: None, enabled: true, separator: false, children }
    }

    #[test]
    fn the_shells_send_to_is_told_by_its_entries_whatever_its_language() {
        let ours = vec!["Mail recipient".to_string(), "Desktop (create shortcut)".to_string(), "Documents".to_string()];
        let kids = |names: &[&str]| names.iter().map(|n| item(n, "", vec![])).collect::<Vec<_>>();
        let send_to = item("Wy&ślij do", "", kids(&["&Mail recipient", "Desktop (create shortcut)", "Documents", "USB (E:)"]));
        assert!(is_shells_send_to(&send_to, &ours));
        let other = item("Bandizip", "", kids(&["Extract here", "Documents"]));
        assert!(!is_shells_send_to(&other, &ours), "one name in common is not enough");
        assert!(!is_shells_send_to(&send_to, &[]), "no menu of our own: the shell's stays");
        let verb = item("Something", "openas", kids(&["Mail recipient", "Documents"]));
        assert!(!is_shells_send_to(&verb, &ours));
    }
}

fn split_shell_items(me: &Me, session: u64, shell: Vec<ShellItem>, background: bool, send_to_names: &[String], cx: &mut App) -> (Pending, Pending, Pending) {
    let is_hidden = |it: &ShellItem| !it.verb.is_empty() && HIDDEN_VERBS.contains(&it.verb.to_ascii_lowercase().as_str());
    let has_child_verb = |it: &ShellItem, verb: &str| it.children.iter().any(|c| c.verb.eq_ignore_ascii_case(verb));

    let mut more_src = Vec::new();
    let mut open_with: Option<Vec<ShellItem>> = None;
    let mut new: Option<Vec<ShellItem>> = None;
    for it in shell {
        if !it.children.is_empty() && has_child_verb(&it, "openas") {
            open_with = Some(it.children.clone());
            continue;
        }
        if !it.children.is_empty() && has_child_verb(&it, "NewFolder") {
            new = Some(it.children.clone());
            continue;
        }
        if it.verb.eq_ignore_ascii_case("openas") && open_with.is_none() {
            // a single "Open with…" entry: say what it does
            open_with = Some(vec![ShellItem { label: t!("Başka bir uygulama seç").into(), ..it.clone() }]);
        }
        if is_hidden(&it) || is_shells_send_to(&it, send_to_names) {
            continue;
        }
        more_src.push(it);
    }
    let more = to_items(me, session, &tidy(more_src), cx);

    let open_with = match open_with {
        Some(v) => Pending::Ready(Rc::new(to_items(me, session, &tidy(v), cx))),
        None => Pending::Ready(Rc::new(vec![Item::action(t!("Başka bir uygulama seç"), icon(L::LayoutGrid), act(me, |v, _, _| v.open_with()))])),
    };
    let new = match new.filter(|_| background) {
        Some(v) => {
            // ours for Folder and Text (inline rename), the shell for every other template; Folder and Shortcut first, then the
            // kinds of file by name (`new_menu_order`)
            let mut rest: Vec<ShellItem> = v
                .into_iter()
                .filter(|c| !c.separator && !c.verb.eq_ignore_ascii_case("NewFolder") && !c.verb.eq_ignore_ascii_case(".txt"))
                .collect();
            let link = rest.iter().position(|c| c.verb.eq_ignore_ascii_case("NewLink")).map(|i| rest.remove(i));
            let mut items = vec![own_new_folder(me)];
            if let Some(link) = link {
                items.extend(to_items(me, session, std::slice::from_ref(&link), cx));
            }
            items.push(Item::Separator);
            let labels: Vec<&str> = rest.iter().map(|c| c.label.as_str()).collect();
            let order = new_menu_order(&labels, t!("Metin belgesi"));
            let mut shell: Vec<Option<Item>> = to_items(me, session, &rest, cx).into_iter().map(Some).collect();
            for o in order {
                match o {
                    Some(i) => items.extend(shell.get_mut(i).and_then(Option::take)),
                    None => items.push(own_new_text(me)),
                }
            }
            Pending::Ready(Rc::new(items))
        }
        None => Pending::Failed,
    };
    (Pending::Ready(Rc::new(more)), open_with, new)
}

/// No leading/trailing/double separators.
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

/// Fills the "more" submenu of an open menu with the shell's items for `target` (sidebar,
/// drives…); `me` receives "Rename" if the shell asks for it.
pub fn load_shell_more(menu: WeakEntity<fluent_menu::MenuView>, target: ShellTarget, me: Me, cx: &mut App) {
    let (tx, rx) = oneshot::channel();
    rf_win::sta::run(move |owner| {
        let _ = tx.send(rf_win::ctxmenu::build(owner, &target, false).map_err(|e| format!("{e:#}")));
    });
    cx.spawn(async move |cx| {
        let r = rx.await.unwrap_or_else(|_| Err("cancelled".into()));
        let _ = cx.update(|cx| {
            let pending = match r {
                Ok((session, shell)) => {
                    let is_hidden = |it: &ShellItem| !it.verb.is_empty() && HIDDEN_VERBS.contains(&it.verb.to_ascii_lowercase().as_str());
                    let keep: Vec<ShellItem> = shell.into_iter().filter(|it| !is_hidden(it)).collect();
                    Pending::Ready(Rc::new(to_items(&me, session, &tidy(keep), cx)))
                }
                Err(e) => {
                    log::warn!("shell menu: {e}");
                    Pending::Failed
                }
            };
            if let Some(m) = menu.upgrade() {
                m.update(cx, |m, cx| m.set_pending("more", pending, cx));
            }
        });
    })
    .detach();
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
                return Item::Sub { label: it.label.clone().into(), icon, items: Sub::Items(Rc::new(to_items(me, session, &tidy(it.children.clone()), cx))), click: None };
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
            log::warn!("shell command: {e:#}");
        }
        let _ = tx.send(r.ok());
    });
    let me = me.clone();
    cx.spawn(async move |cx| {
        let outcome = rx.await;
        let _ = me.update(cx, |v, cx| {
            let shell_folder = matches!(v.location(), Location::Shell { .. });
            match outcome {
                // a phone's folder has no inline rename here
                Ok(Some(MenuOutcome::Rename)) if !shell_folder => {
                    v.request_rename();
                    cx.notify();
                }
                // what the shell did on a device (a deletion, a new folder) isn't watched: list again
                Ok(Some(MenuOutcome::Invoked(_) | MenuOutcome::Rename)) if shell_folder => v.refresh(cx),
                _ => {}
            }
        });
    })
    .detach();
}

fn shift_down() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_SHIFT};
    unsafe { GetKeyState(VK_SHIFT.0 as i32) < 0 }
}

impl FolderView {
    /// Right-click in a shell folder (a phone, Libraries…): our menu, filled with the shell's
    /// own commands for the selection or the folder (found by parsing names, as they have no
    /// path). Shift gives the classic menu instead, like elsewhere.
    fn show_shell_fluent_menu(&mut self, background: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Location::Shell { parsing, .. } = self.location().clone() else { return };
        let mut sel: Vec<u32> = self.selected.iter().copied().collect();
        sel.sort_unstable();
        let items: Vec<PathBuf> = if background { Vec::new() } else { sel.iter().filter_map(|&i| self.shell_items.get(i as usize)).map(|c| PathBuf::from(&c.parsing)).collect() };
        let background = items.is_empty();
        let me = cx.entity().downgrade();
        let mut menu = Menu::default();
        if !background {
            menu.quick.push(Quick::new(L::Copy, t!("Kopyala  (Ctrl+C)"), true, act(&me, |v, w, cx| v.copy_to_clipboard(Transfer::Copy, w, cx))));
        }
        menu.items.push(Item::Sub { label: t!("Diğer seçenekleri göster").into(), icon: icon(L::Ellipsis), items: Sub::Pending { key: "more", fallback: None }, click: None });
        if background {
            menu.items.push(Item::Separator);
            menu.items.push(Item::action(t!("Yenile"), icon(L::RotateCw), act(&me, |v, _, cx| v.refresh(cx))).hint("Ctrl+R"));
        }
        let menu_view = fluent_menu::show_context(menu, window.mouse_position(), window, cx);
        let target = match items.as_slice() {
            [] => ShellTarget::Background(PathBuf::from(&parsing)),
            [one] => ShellTarget::One(one.clone()),
            _ => ShellTarget::Items(items),
        };
        let (tx, rx) = oneshot::channel();
        rf_win::sta::run(move |owner| {
            let _ = tx.send(rf_win::ctxmenu::build(owner, &target, false).map_err(|e| format!("{e:#}")));
        });
        let menu_weak = menu_view.downgrade();
        cx.spawn(async move |_, cx| {
            let result = rx.await.unwrap_or_else(|_| Err("cancelled".into()));
            let _ = cx.update(|cx| {
                let more = match result {
                    Ok((session, shell)) => Pending::Ready(Rc::new(to_items(&me, session, &tidy(shell), cx))),
                    Err(e) => {
                        log::info!("shell folder menu: {e}");
                        Pending::Failed
                    }
                };
                if let Some(m) = menu_weak.upgrade() {
                    m.update(cx, |m, cx| {
                        m.set_pending("more", more, cx);
                        m.inline_pending("more", cx);
                    });
                }
            });
        })
        .detach();
    }
}

/// The file to make the desktop background of `p`: `p` itself, or for a picture inside an archive a copy of it in the app's
/// `wallpapers` folder (one file per name, replaced when the same name is chosen again). Blocks (extraction): a worker thread.
fn wallpaper_file(p: &std::path::Path) -> Option<std::path::PathBuf> {
    if p.exists() || crate::archive_view::split(p).is_none() {
        return Some(p.to_path_buf());
    }
    let extracted = crate::archive_view::real_copy(p)?;
    let dir = crate::settings::app_dir()?.join("wallpapers");
    std::fs::create_dir_all(&dir).ok()?;
    let to = dir.join(p.file_name()?);
    std::fs::copy(&extracted, &to).ok()?;
    Some(to)
}

#[cfg(test)]
mod new_menu_tests {
    /// The New menu's kinds of file go by their names, ours among the shell's (issue 369).
    #[test]
    fn the_new_menu_lists_kinds_of_file_by_name() {
        use super::new_menu_order;
        // as the shell gives them: by extension
        let shell = ["Bit eşlem resmi", "CorelDRAW 2026 Graphic", "Google Docs", "Sıkıştırılmış Klasör", "akıllı not"];
        let order = new_menu_order(&shell, "Metin belgesi");
        let names: Vec<&str> = order.iter().map(|o| o.map_or("Metin belgesi", |i| shell[i])).collect();
        assert_eq!(names, ["akıllı not", "Bit eşlem resmi", "CorelDRAW 2026 Graphic", "Google Docs", "Metin belgesi", "Sıkıştırılmış Klasör"]);
        assert_eq!(new_menu_order(&[], "Text document"), [None], "no templates: ours alone");
    }
}
