//! Pinned commands and the Windows program launch shared by the toolbar and file drops.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{WindowExt as _, h_flex, v_flex};
use gpui_kit::*;
use serde::{Deserialize, Serialize};

use crate::i18n::tx;
use crate::program_launch::{accepts_program, launch, launch_error, split_arguments};
use crate::settings::{self, Settings};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickCommand {
    #[serde(default)]
    pub label: String,
    pub program: String,
    #[serde(default)]
    pub arguments: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tooltip: String,
    #[serde(default)]
    pub in_menu: bool,
}

impl QuickCommand {
    pub(crate) fn button_label(&self) -> Option<&str> {
        let label = self.label.trim();
        (!label.is_empty()).then_some(label)
    }

    pub(crate) fn name(&self) -> &str {
        self.button_label().unwrap_or_else(|| {
            let program = self.program.trim().trim_matches('"');
            Path::new(program).file_stem().and_then(OsStr::to_str).unwrap_or(program)
        })
    }

    pub(crate) fn tooltip_text(&self) -> &str {
        let tip = self.tooltip.trim();
        if tip.is_empty() { self.name() } else { tip }
    }

    pub(crate) fn menu_label(&self) -> &str {
        self.button_label().unwrap_or_else(|| self.tooltip_text())
    }

    pub(crate) fn icon_path(&self) -> Option<String> {
        let path = expand_path(self.icon.as_deref().unwrap_or(&self.program));
        Path::new(&path).is_absolute().then_some(path)
    }

    fn arguments(&self, folder: Option<&Path>, selected: &[PathBuf]) -> anyhow::Result<(Vec<OsString>, bool)> {
        let mut arguments = Vec::new();
        let mut uses_paths = false;
        for token in split_arguments(&self.arguments)? {
            match token.to_str() {
                Some("{folder}") => {
                    uses_paths = true;
                    arguments.push(folder.ok_or_else(|| anyhow::anyhow!("{}", tx("Önce bir klasör açın.", "Open a folder first.")))?.as_os_str().to_owned());
                }
                Some("{selection}" | "%sel%") => {
                    uses_paths = true;
                    anyhow::ensure!(!selected.is_empty(), "{}", tx("Önce dosyaları seçin.", "Select files first."));
                    arguments.extend(selected.iter().map(|p| p.as_os_str().to_owned()));
                }
                _ => arguments.push(token),
            }
        }
        Ok((arguments, uses_paths))
    }
}

fn expand_path(text: &str) -> String {
    rf_core::addrcmd::expand_vars(text.trim().trim_matches('"'), &|key| std::env::var(key).ok())
}

fn valid_program(program: &str) -> bool {
    let expanded = expand_path(program);
    !expanded.is_empty() && (!rf_core::addrcmd::has_dir_part(&expanded) || Path::new(&expanded).is_absolute())
}

pub(crate) fn run(command: QuickCommand, folder: Option<PathBuf>, selection: Vec<PathBuf>, window: &mut Window, cx: &mut App) {
    let (arguments, uses_paths) = match command.arguments(folder.as_deref(), &selection) {
        Ok(args) => args,
        Err(err) => return launch_error(err, window, cx),
    };
    let window = window.window_handle();
    let (tx_result, result) = futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let run = || {
            let program = expand_path(&command.program);
            let path = PathBuf::from(&program);
            let exe = if path.is_absolute() && accepts_program(&path) {
                path
            } else {
                let line = rf_core::addrcmd::CommandLine { program, args: String::new() };
                rf_win::runfind::find(&[line], Path::new(""))
                    .ok_or_else(|| anyhow::anyhow!("{}", tx("Program bulunamadı.", "Program not found.")))?.exe
            };
            launch(exe, arguments, uses_paths, folder)
        };
        let _ = tx_result.send(run());
    });
    cx.spawn(async move |cx| match result.await {
        Ok(Ok(Some(folder))) => {
            cx.update(|cx| crate::workspace::route_ipc_shown(folder.to_string_lossy().into_owned(), cx));
        }
        Ok(Err(err)) => {
            let _ = window.update(cx, |_, window, cx| launch_error(err, window, cx));
        }
        _ => {}
    }).detach();
}

pub(crate) fn edit(index: Option<usize>, window: &mut Window, cx: &mut App) {
    let current = index.and_then(|i| cx.global::<Settings>().toolbar_commands.get(i)).cloned();
    let initial = current.clone().unwrap_or(QuickCommand {
        label: String::new(), program: String::new(), arguments: String::new(), icon: None, tooltip: String::new(), in_menu: false,
    });
    let label = cx.new(|cx| InputState::new(window, cx).default_value(initial.label).placeholder(tx("Düğme adı", "Button label")));
    let tooltip = cx.new(|cx| InputState::new(window, cx).default_value(initial.tooltip)
        .placeholder(tx("Düğme veya program adı", "Button or program name")));
    let program = cx.new(|cx| InputState::new(window, cx).default_value(initial.program).placeholder(r"C:\Tools\tool.exe"));
    let args = cx.new(|cx| InputState::new(window, cx).default_value(initial.arguments).placeholder("--input {selection}"));
    let icon = cx.new(|cx| InputState::new(window, cx).default_value(initial.icon.unwrap_or_default())
        .placeholder(tx("Program simgesini kullan", "Use the program's icon")));
    let error: Entity<Option<String>> = cx.new(|_| None);
    let in_menu = cx.new(|_| initial.in_menu);
    let initial_focus = std::cell::Cell::new(true);
    window.open_alert_dialog(cx, move |dialog, window, cx| {
        if initial_focus.replace(false) {
            label.focus_handle(cx).focus(window, cx);
        }
        let (label_in, program_in, args_in, icon_in, err, current) = (label.clone(), program.clone(), args.clone(), icon.clone(), error.clone(), current.clone());
        let tooltip_in = tooltip.clone();
        let menu_in = in_menu.clone();
        let menu_flag = in_menu.clone();
        let palette = crate::theme::Palette::for_window(window);
        let browse = {
            let icon = icon.clone();
            crate::settings_view::button("browse-command-icon", &palette, tx("Gözat…", "Browse…"), move |window, cx| pick_icon(icon.clone(), window, cx))
        };
        dialog
            .title(tx("Araç çubuğu komutu", "Toolbar command"))
            .bg(palette.card_bg)
            .width(px(500.))
            .show_cancel(true)
            .child(v_flex().gap_2()
                .child(h_flex().gap_2()
                    .child(v_flex().gap_2().flex_1().min_w_0()
                        .child(tx("Düğme adı (isteğe bağlı)", "Button label (optional)"))
                        .child(Input::new(&label)))
                    .child(v_flex().gap_2().flex_1().min_w_0()
                        .child(tx("Araç ipucu (isteğe bağlı)", "Tooltip (optional)"))
                        .child(Input::new(&tooltip))))
                .child(tx("Program yolu veya PATH üzerindeki adı", "Program path or name on PATH"))
                .child(Input::new(&program))
                .child(tx("Bağımsız değişkenler (isteğe bağlı)", "Arguments (optional)"))
                .child(Input::new(&args))
                .child(div().text_sm().child(tx(
                    "{folder} açık klasörü, {selection} seçilen her yolu ayrı bir bağımsız değişken olarak ekler.",
                    "{folder} passes the current folder. {selection} passes each selected path as a separate argument.",
                )))
                .child(tx("Simge (isteğe bağlı)", "Icon (optional)"))
                .child(h_flex().items_center().gap_2().child(div().flex_1().min_w_0().child(Input::new(&icon))).child(browse))
                .child(h_flex()
                    .w_full()
                    .pt_1()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(div().text_sm().text_color(palette.text).child(tx("Açılır menüde göster", "Show in menu")))
                    .child(crate::settings_view::switch("command-in-menu", &palette, *in_menu.read(cx), move |_, cx| {
                        menu_flag.update(cx, |value, cx| {
                            *value = !*value;
                            cx.notify();
                        });
                        cx.refresh_windows();
                    })))
                .children(error.read(cx).clone().map(|msg| div().text_color(rgb(0xd84949)).child(msg))))
            .on_ok(move |_, _, cx| {
                let label = label_in.read(cx).value().trim().to_owned();
                let program = program_in.read(cx).value().trim().trim_matches('"').to_owned();
                let icon = icon_in.read(cx).value().trim().trim_matches('"').to_owned();
                let message = if !valid_program(&program) {
                    Some(tx("Programın adını veya tam yolunu girin.", "Enter a program name or absolute path."))
                } else if !icon.is_empty() && !Path::new(&expand_path(&icon)).is_absolute() {
                    Some(tx("Simge dosyasının tam yolunu girin.", "Enter the full path to an icon file."))
                } else {
                    None
                };
                if let Some(message) = message {
                    err.update(cx, |error, cx| {
                        *error = Some(message.into());
                        cx.notify();
                    });
                    cx.refresh_windows();
                    return false;
                }
                let command = QuickCommand {
                    label, program, arguments: args_in.read(cx).value().to_string(),
                    icon: (!icon.is_empty()).then_some(icon), tooltip: tooltip_in.read(cx).value().trim().to_owned(),
                    in_menu: *menu_in.read(cx),
                };
                settings::update(cx, |s| match index {
                    Some(i) if s.toolbar_commands.get(i) == current.as_ref() => s.toolbar_commands[i] = command,
                    Some(_) => {}
                    None => s.toolbar_commands.push(command),
                });
                true
            })
    });
}

fn pick_icon(input: Entity<InputState>, window: &Window, cx: &mut App) {
    let window = window.window_handle();
    let (send, receive) = futures::channel::oneshot::channel();
    let title = tx("Simge seçin", "Choose an icon");
    let files = tx("Simge dosyaları", "Icon files");
    rf_win::sta::run(move |owner| {
        let _ = send.send(rf_win::dialogs::pick(owner, title, rf_win::dialogs::Pick::File { filters: &[(files, "*.ico;*.png;*.exe;*.lnk")] }));
    });
    cx.spawn(async move |cx| {
        if let Ok(Some(path)) = receive.await {
            let _ = window.update(cx, |_, window, cx| input.update(cx, |input, cx| {
                input.set_value(path.to_string_lossy().into_owned(), window, cx);
            }));
        }
    }).detach();
}

pub(crate) fn controls(index: usize, command: QuickCommand, p: &crate::theme::Palette) -> impl IntoElement {
    use crate::settings_view::button;
    h_flex().gap_2()
        .child(button(("edit-quick-command", index), p, tx("Düzenle", "Edit"), move |window, cx| edit(Some(index), window, cx)))
        .child(button(("remove-quick-command", index), p, tx("Kaldır", "Remove"), move |_, cx| {
            settings::update(cx, |s| {
                if s.toolbar_commands.get(index) == Some(&command) {
                    s.toolbar_commands.remove(index);
                }
            });
        }))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::Path;

    use super::{QuickCommand, valid_program};

    #[test]
    fn an_icon_only_command_does_not_require_a_button_label() {
        let mut command: QuickCommand = serde_json::from_str(r#"{"program":"C:\\Tools\\update_all.bat","tooltip":"Update tools"}"#).unwrap();
        assert_eq!(command.button_label(), None);
        assert_eq!(command.name(), "update_all");
        assert_eq!(command.tooltip_text(), "Update tools");
        assert_eq!(command.menu_label(), "Update tools");
        command.tooltip.clear();
        assert_eq!(command.tooltip_text(), "update_all");
    }

    #[test]
    fn toolbar_accepts_environment_paths_and_commands_saved_before_icons() {
        let command: QuickCommand = serde_json::from_str(r#"{"label":"Editor","program":"%SystemRoot%\\notepad.exe","arguments":"\"%sel%\""}"#).unwrap();
        assert!(valid_program(&command.program));
        assert!(!valid_program(r"relative\tool.exe"));
        assert!(command.icon.is_none());
        assert_eq!(command.button_label(), Some("Editor"));
        assert_eq!(command.tooltip_text(), "Editor");
        assert!(!command.in_menu);
        assert_eq!(command.icon_path(), Some(format!(r"{}\notepad.exe", std::env::var("SystemRoot").unwrap())));
        let path = Path::new(r"C:\A folder\file.txt").to_path_buf();
        assert_eq!(command.arguments(None, std::slice::from_ref(&path)).unwrap(), (vec![path.into_os_string()], true));
    }

    #[test]
    fn placeholders_keep_spaces_and_expand_to_separate_arguments() {
        let command = QuickCommand { label: "Tool".into(), program: "tool.exe".into(), arguments: r#"--name "two words" {folder} {selection}"#.into(), icon: None, tooltip: String::new(), in_menu: false };
        let folder = Path::new(r"C:\A folder\");
        let files = [folder.join("one & two.txt"), folder.join("три.txt")];
        let (arguments, uses_paths) = command.arguments(Some(folder), &files).unwrap();
        assert!(uses_paths);
        assert_eq!(arguments,
            [OsString::from("--name"), OsString::from("two words"), folder.as_os_str().to_owned(), files[0].clone().into_os_string(), files[1].clone().into_os_string()]);
        assert!(command.arguments(Some(folder), &[]).is_err());
        assert!(command.arguments(None, &files).is_err());
    }

    #[test]
    fn putting_a_command_in_the_menu_survives_a_settings_round_trip() {
        let command: QuickCommand = toml::from_str("label = 'My tool'\nprogram = 'tool.exe'\ntooltip = 'Run the tool'\nin_menu = true").unwrap();
        let saved = toml::to_string(&command).unwrap();
        let restored: QuickCommand = toml::from_str(&saved).unwrap();
        assert_eq!(command, restored);
        assert!(restored.in_menu);
        assert_eq!(restored.menu_label(), "My tool");
    }
}
