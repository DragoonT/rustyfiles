//! File categorization → symbol icon (Lucide) + type label.
//! Native shell icons/thumbnails come later via rf-win; this is the instant, zero-I/O fallback.

use std::borrow::Cow;

use crate::t;
use gpui_kit::assets::IconName as L;
use gpui_kit::component::Icon;
use gpui_kit::{
    App, AnyElement, AssetSource, IntoElement, ParentElement, Pixels, SharedString, Styled, div, rgb,
};
use rf_core::Entry;

const WINDOWS_THEME_ASSETS: &[(&str, &[u8])] = &[
    (
        "windows-theme/add.svg",
        include_bytes!("../assets/windows-theme/add.svg"),
    ),
    (
        "windows-theme/app.svg",
        include_bytes!("../assets/windows-theme/app.svg"),
    ),
    (
        "windows-theme/back.svg",
        include_bytes!("../assets/windows-theme/back.svg"),
    ),
    (
        "windows-theme/chart.svg",
        include_bytes!("../assets/windows-theme/chart.svg"),
    ),
    (
        "windows-theme/check.svg",
        include_bytes!("../assets/windows-theme/check.svg"),
    ),
    (
        "windows-theme/cloud.svg",
        include_bytes!("../assets/windows-theme/cloud.svg"),
    ),
    (
        "windows-theme/chevron-down.svg",
        include_bytes!("../assets/windows-theme/chevron-down.svg"),
    ),
    (
        "windows-theme/chevron-left.svg",
        include_bytes!("../assets/windows-theme/chevron-left.svg"),
    ),
    (
        "windows-theme/chevron-right.svg",
        include_bytes!("../assets/windows-theme/chevron-right.svg"),
    ),
    (
        "windows-theme/chevron-up.svg",
        include_bytes!("../assets/windows-theme/chevron-up.svg"),
    ),
    (
        "windows-theme/close.svg",
        include_bytes!("../assets/windows-theme/close.svg"),
    ),
    (
        "windows-theme/context-compress.svg",
        include_bytes!("../assets/windows-theme/context-compress.svg"),
    ),
    (
        "windows-theme/context-copy-path.svg",
        include_bytes!("../assets/windows-theme/context-copy-path.svg"),
    ),
    (
        "windows-theme/context-copy.svg",
        include_bytes!("../assets/windows-theme/context-copy.svg"),
    ),
    (
        "windows-theme/context-cut.svg",
        include_bytes!("../assets/windows-theme/context-cut.svg"),
    ),
    (
        "windows-theme/context-delete.svg",
        include_bytes!("../assets/windows-theme/context-delete.svg"),
    ),
    (
        "windows-theme/context-new-folder.svg",
        include_bytes!("../assets/windows-theme/context-new-folder.svg"),
    ),
    (
        "windows-theme/context-open-with.svg",
        include_bytes!("../assets/windows-theme/context-open-with.svg"),
    ),
    (
        "windows-theme/context-paste.svg",
        include_bytes!("../assets/windows-theme/context-paste.svg"),
    ),
    (
        "windows-theme/context-properties.svg",
        include_bytes!("../assets/windows-theme/context-properties.svg"),
    ),
    (
        "windows-theme/context-rename.svg",
        include_bytes!("../assets/windows-theme/context-rename.svg"),
    ),
    (
        "windows-theme/context-share.svg",
        include_bytes!("../assets/windows-theme/context-share.svg"),
    ),
    (
        "windows-theme/context-shortcut.svg",
        include_bytes!("../assets/windows-theme/context-shortcut.svg"),
    ),
    (
        "windows-theme/copy.svg",
        include_bytes!("../assets/windows-theme/copy.svg"),
    ),
    (
        "windows-theme/cut.svg",
        include_bytes!("../assets/windows-theme/cut.svg"),
    ),
    (
        "windows-theme/delete.svg",
        include_bytes!("../assets/windows-theme/delete.svg"),
    ),
    (
        "windows-theme/details.svg",
        include_bytes!("../assets/windows-theme/details.svg"),
    ),
    (
        "windows-theme/down.svg",
        include_bytes!("../assets/windows-theme/down.svg"),
    ),
    (
        "windows-theme/download.svg",
        include_bytes!("../assets/windows-theme/download.svg"),
    ),
    (
        "windows-theme/drive.svg",
        include_bytes!("../assets/windows-theme/drive.svg"),
    ),
    (
        "windows-theme/external-link.svg",
        include_bytes!("../assets/windows-theme/external-link.svg"),
    ),
    (
        "windows-theme/eye.svg",
        include_bytes!("../assets/windows-theme/eye.svg"),
    ),
    (
        "windows-theme/eye-off.svg",
        include_bytes!("../assets/windows-theme/eye-off.svg"),
    ),
    (
        "windows-theme/file.svg",
        include_bytes!("../assets/windows-theme/file.svg"),
    ),
    (
        "windows-theme/file-archive.svg",
        include_bytes!("../assets/windows-theme/file-archive.svg"),
    ),
    (
        "windows-theme/file-audio.svg",
        include_bytes!("../assets/windows-theme/file-audio.svg"),
    ),
    (
        "windows-theme/file-code.svg",
        include_bytes!("../assets/windows-theme/file-code.svg"),
    ),
    (
        "windows-theme/file-image.svg",
        include_bytes!("../assets/windows-theme/file-image.svg"),
    ),
    (
        "windows-theme/file-sheet.svg",
        include_bytes!("../assets/windows-theme/file-sheet.svg"),
    ),
    (
        "windows-theme/file-text.svg",
        include_bytes!("../assets/windows-theme/file-text.svg"),
    ),
    (
        "windows-theme/file-video.svg",
        include_bytes!("../assets/windows-theme/file-video.svg"),
    ),
    (
        "windows-theme/filter.svg",
        include_bytes!("../assets/windows-theme/filter.svg"),
    ),
    (
        "windows-theme/folder.svg",
        include_bytes!("../assets/windows-theme/folder.svg"),
    ),
    (
        "windows-theme/folder-open.svg",
        include_bytes!("../assets/windows-theme/folder-open.svg"),
    ),
    (
        "windows-theme/forward.svg",
        include_bytes!("../assets/windows-theme/forward.svg"),
    ),
    (
        "windows-theme/grid.svg",
        include_bytes!("../assets/windows-theme/grid.svg"),
    ),
    (
        "windows-theme/home.svg",
        include_bytes!("../assets/windows-theme/home.svg"),
    ),
    (
        "windows-theme/home-roof.svg",
        include_bytes!("../assets/windows-theme/home-roof.svg"),
    ),
    (
        "windows-theme/list.svg",
        include_bytes!("../assets/windows-theme/list.svg"),
    ),
    (
        "windows-theme/lock.svg",
        include_bytes!("../assets/windows-theme/lock.svg"),
    ),
    (
        "windows-theme/menu.svg",
        include_bytes!("../assets/windows-theme/menu.svg"),
    ),
    (
        "windows-theme/monitor.svg",
        include_bytes!("../assets/windows-theme/monitor.svg"),
    ),
    (
        "windows-theme/minus.svg",
        include_bytes!("../assets/windows-theme/minus.svg"),
    ),
    (
        "windows-theme/network.svg",
        include_bytes!("../assets/windows-theme/network.svg"),
    ),
    (
        "windows-theme/nav-back.svg",
        include_bytes!("../assets/windows-theme/nav-back.svg"),
    ),
    (
        "windows-theme/nav-forward.svg",
        include_bytes!("../assets/windows-theme/nav-forward.svg"),
    ),
    (
        "windows-theme/new-file.svg",
        include_bytes!("../assets/windows-theme/new-file.svg"),
    ),
    (
        "windows-theme/new-folder.svg",
        include_bytes!("../assets/windows-theme/new-folder.svg"),
    ),
    (
        "windows-theme/optical-drive.svg",
        include_bytes!("../assets/windows-theme/optical-drive.svg"),
    ),
    (
        "windows-theme/panel.svg",
        include_bytes!("../assets/windows-theme/panel.svg"),
    ),
    (
        "windows-theme/paste.svg",
        include_bytes!("../assets/windows-theme/paste.svg"),
    ),
    (
        "windows-theme/pin.svg",
        include_bytes!("../assets/windows-theme/pin.svg"),
    ),
    (
        "windows-theme/properties.svg",
        include_bytes!("../assets/windows-theme/properties.svg"),
    ),
    (
        "windows-theme/refresh.svg",
        include_bytes!("../assets/windows-theme/refresh.svg"),
    ),
    (
        "windows-theme/removable-drive.svg",
        include_bytes!("../assets/windows-theme/removable-drive.svg"),
    ),
    (
        "windows-theme/rename.svg",
        include_bytes!("../assets/windows-theme/rename.svg"),
    ),
    (
        "windows-theme/share.svg",
        include_bytes!("../assets/windows-theme/share.svg"),
    ),
    (
        "windows-theme/sort.svg",
        include_bytes!("../assets/windows-theme/sort.svg"),
    ),
    (
        "windows-theme/up.svg",
        include_bytes!("../assets/windows-theme/up.svg"),
    ),
    (
        "windows-theme/upload.svg",
        include_bytes!("../assets/windows-theme/upload.svg"),
    ),
    (
        "windows-theme/warning.svg",
        include_bytes!("../assets/windows-theme/warning.svg"),
    ),
];

pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        if let Some((_, bytes)) = WINDOWS_THEME_ASSETS.iter().find(|(name, _)| *name == path) {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        gpui_kit::assets::AllAssets.load(path)
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        let mut files = gpui_kit::assets::AllAssets.list(path)?;
        files.extend(
            WINDOWS_THEME_ASSETS
                .iter()
                .filter_map(|(name, _)| name.starts_with(path).then(|| (*name).into())),
        );
        Ok(files)
    }
}

pub fn install_theme_resolver(cx: &mut App) {
    gpui_kit::component::set_icon_resolver(cx, |path, cx| {
        let enabled = cx
            .try_global::<crate::settings::Settings>()
            .is_some_and(|s| s.windows_file_explorer_theme);
        resolved_windows_theme_path(path, enabled).map(Into::into)
    });
}

pub fn original_icon(icon: L, size: Pixels) -> Icon {
    let path = icon.path();
    match gpui_kit::assets::AllAssets.load(&path) {
        Ok(Some(bytes)) => Icon::default().data(&bytes).size(size),
        Ok(None) => {
            log::warn!("missing original icon asset: {path}");
            Icon::default().size(size)
        }
        Err(error) => {
            log::warn!("could not load original icon asset {path}: {error:#}");
            Icon::default().size(size)
        }
    }
}

pub fn windows_home_icon(size: Pixels) -> AnyElement {
    div()
        .relative()
        .size(size)
        .flex_shrink_0()
        .child(
            Icon::default()
                .path("windows-theme/home.svg")
                .size(size)
                .text_color(rgb(0xC5C6C8))
                .into_any_element(),
        )
        .child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    Icon::default()
                        .path("windows-theme/home-roof.svg")
                        .size(size)
                        .text_color(rgb(0xF27B00))
                        .into_any_element(),
                ),
        )
        .into_any_element()
}

fn resolved_windows_theme_path(path: &str, enabled: bool) -> Option<&'static str> {
    enabled.then(|| windows_theme_path(path)).flatten()
}

fn windows_theme_path(path: &str) -> Option<&'static str> {
    let name = path.strip_prefix("icons/")?.strip_suffix(".svg")?;
    let asset = match name {
        "activity" => "refresh",
        "a-large-small" | "text-align-justify" | "type" => "file-text",
        "archive" => "file-archive",
        "arrow-down" => "down",
        "arrow-down-up" | "arrow-down-wide-narrow" | "arrow-up-down" | "chevrons-up-down" => "sort",
        "arrow-left" => "back",
        "arrow-right" | "move-right" => "forward",
        "arrow-up" => "up",
        "arrow-up-right" | "external-link" | "square-arrow-out-up-right" => "external-link",
        "app-window" | "file-type" | "maximize" | "maximize-2" => "app",
        "check" | "check-check" | "check-circle" | "check-circle-2" | "check-square"
        | "circle-check" | "square-check" => "check",
        "circle-alert" | "circle-warning" => "warning",
        "circle-slash" => "close",
        "chevron-down" | "chevrons-down" => "chevron-down",
        "chevron-left" | "chevrons-left" => "chevron-left",
        "chevron-right" | "chevrons-right" => "chevron-right",
        "chevron-up" | "chevrons-up" => "chevron-up",
        "clipboard-copy" | "copy" | "copy-check" | "copy-plus" => "copy",
        "clipboard-paste" | "clipboard" | "clipboard-check" => "paste",
        "cloud" => "cloud",
        "code" => "file-code",
        "columns-2"
        | "columns-3"
        | "layout-panel-left"
        | "panel-bottom"
        | "panel-top"
        | "picture-in-picture"
        | "picture-in-picture-2"
        | "rows-2"
        | "rows-3"
        | "square-split-horizontal" => "panel",
        "disc" | "disc-3" => "optical-drive",
        "eject" => "removable-drive",
        "ellipsis" | "square-menu" => "menu",
        "ethernet-port" => "network",
        "eye" => "eye",
        "eye-off" => "eye-off",
        "file" | "file-pen" | "file-edit" | "files" => "file",
        "file-archive" => "file-archive",
        "file-audio" | "file-music" | "headphones" | "music" => "file-audio",
        "file-code" => "file-code",
        "file-image" | "camera" => "file-image",
        "file-spreadsheet" | "file-chart-column" => "file-sheet",
        "file-text" | "file-type-2" | "file-check" | "scroll-text" => "file-text",
        "file-video" | "file-video-camera" => "file-video",
        "video" => "file-video",
        "filter" | "list-filter" => "filter",
        "folder" | "folder-check" | "folder-cog" | "folder-down" | "folder-input"
        | "folder-output" | "folder-search" | "folder-sync" | "folder-tree" | "folder-up"
        | "folder-x" => "folder",
        "folder-open" | "folder-closed" => "folder-open",
        "folder-plus-2" => "new-folder",
        "forward" | "skip-forward" | "arrow-right-circle" => "forward",
        "hard-drive" | "hard-drive-download" => "drive",
        "house" | "home" => "home",
        "image" | "images" => "file-image",
        "layout-grid" | "grid-2x2" | "layout-dashboard" => "grid",
        "layout-list" | "list" | "list-checks" | "list-tree" => "list",
        "library" => "folder",
        "link" => "external-link",
        "lock" | "shield-alert" | "shield-check" => "lock",
        "menu" => "menu",
        "panel-left" | "panel-left-close" | "panel-left-open" | "panel-right"
        | "panel-right-close" | "panel-right-open" => "panel",
        "monitor" | "monitor-cog" | "projector" => "monitor",
        "network" | "server" | "wifi" | "wifi-high" | "wifi-low" | "wifi-off" | "wifi-zero"
        | "globe" | "cast" => "network",
        "file-plus" | "file-plus-2" => "new-file",
        "folder-plus" => "new-folder",
        "square-chevron-right" => "chevron-right",
        "plus" | "square-plus" | "circle-plus" | "zoom-in" => "add",
        "minus" | "zoom-out" => "minus",
        "refresh-cw" | "refresh-ccw" | "rotate-cw" | "rotate-ccw" | "rotate-cw-square" => "refresh",
        "pencil" | "pencil-line" | "file-pen-line" | "edit" | "replace" => "rename",
        "pin" | "pin-off" => "pin",
        "share" | "share-2" => "share",
        "scissors" | "cut" => "cut",
        "save" => "file",
        "skip-back" => "back",
        "trash" | "trash-2" | "trash-off" => "delete",
        "usb" | "usb-drive" => "removable-drive",
        "upload" | "cloud-upload" => "upload",
        "download" | "cloud-download" => "download",
        "chart-pie" => "chart",
        "triangle-alert" | "alert-triangle" | "alert-circle" => "warning",
        "x" | "circle-x" | "circle-xmark" => "close",
        _ => return None,
    };
    Some(match asset {
        "add" => "windows-theme/add.svg",
        "app" => "windows-theme/app.svg",
        "back" => "windows-theme/back.svg",
        "check" => "windows-theme/check.svg",
        "chart" => "windows-theme/chart.svg",
        "cloud" => "windows-theme/cloud.svg",
        "chevron-down" => "windows-theme/chevron-down.svg",
        "chevron-left" => "windows-theme/chevron-left.svg",
        "chevron-right" => "windows-theme/chevron-right.svg",
        "chevron-up" => "windows-theme/chevron-up.svg",
        "close" => "windows-theme/close.svg",
        "copy" => "windows-theme/copy.svg",
        "cut" => "windows-theme/cut.svg",
        "delete" => "windows-theme/delete.svg",
        "down" => "windows-theme/down.svg",
        "download" => "windows-theme/download.svg",
        "drive" => "windows-theme/drive.svg",
        "external-link" => "windows-theme/external-link.svg",
        "eye" => "windows-theme/eye.svg",
        "eye-off" => "windows-theme/eye-off.svg",
        "file" => "windows-theme/file.svg",
        "file-archive" => "windows-theme/file-archive.svg",
        "file-audio" => "windows-theme/file-audio.svg",
        "file-code" => "windows-theme/file-code.svg",
        "file-image" => "windows-theme/file-image.svg",
        "file-sheet" => "windows-theme/file-sheet.svg",
        "file-text" => "windows-theme/file-text.svg",
        "file-video" => "windows-theme/file-video.svg",
        "filter" => "windows-theme/filter.svg",
        "folder" => "windows-theme/folder.svg",
        "folder-open" => "windows-theme/folder-open.svg",
        "forward" => "windows-theme/forward.svg",
        "grid" => "windows-theme/grid.svg",
        "home" => "windows-theme/home.svg",
        "list" => "windows-theme/list.svg",
        "lock" => "windows-theme/lock.svg",
        "menu" => "windows-theme/menu.svg",
        "panel" => "windows-theme/panel.svg",
        "monitor" => "windows-theme/monitor.svg",
        "minus" => "windows-theme/minus.svg",
        "network" => "windows-theme/network.svg",
        "new-file" => "windows-theme/new-file.svg",
        "new-folder" => "windows-theme/new-folder.svg",
        "optical-drive" => "windows-theme/optical-drive.svg",
        "paste" => "windows-theme/paste.svg",
        "pin" => "windows-theme/pin.svg",
        "properties" => "windows-theme/properties.svg",
        "refresh" => "windows-theme/refresh.svg",
        "removable-drive" => "windows-theme/removable-drive.svg",
        "rename" => "windows-theme/rename.svg",
        "share" => "windows-theme/share.svg",
        "sort" => "windows-theme/sort.svg",
        "up" => "windows-theme/up.svg",
        "upload" => "windows-theme/upload.svg",
        "warning" => "windows-theme/warning.svg",
        _ => return None,
    })
}

pub fn windows_theme_asset_path(icon: L) -> Option<&'static str> {
    let path = icon.path();
    windows_theme_path(&path)
}

pub fn windows_theme_asset_bytes(path: &str) -> Option<&'static [u8]> {
    WINDOWS_THEME_ASSETS.iter().find(|(name, _)| *name == path).map(|(_, bytes)| *bytes)
}

/// Windows 11's Segoe Fluent Icons glyphs for standard context-menu actions.
pub fn windows_theme_context_glyph(path: &str) -> Option<char> {
    let glyph = match path {
        "windows-theme/add.svg" => 0xE710,
        "windows-theme/back.svg" => 0xE72B,
        "windows-theme/check.svg" => 0xE73E,
        "windows-theme/context-cut.svg" | "windows-theme/cut.svg" => 0xE8C6,
        "windows-theme/context-copy.svg"
        | "windows-theme/context-copy-path.svg"
        | "windows-theme/copy.svg" => 0xE8C8,
        "windows-theme/context-paste.svg" | "windows-theme/paste.svg" => 0xE77F,
        "windows-theme/context-rename.svg" | "windows-theme/rename.svg" => 0xE8AC,
        "windows-theme/context-share.svg" | "windows-theme/share.svg" => 0xE72D,
        "windows-theme/context-delete.svg" | "windows-theme/delete.svg" => 0xE74D,
        "windows-theme/context-open-with.svg" => 0xE7AC,
        "windows-theme/context-properties.svg" | "windows-theme/properties.svg" => 0xE946,
        "windows-theme/context-shortcut.svg" => 0xE71B,
        "windows-theme/context-new-folder.svg" | "windows-theme/new-folder.svg" => 0xE8F4,
        "windows-theme/details.svg" => 0xE890,
        "windows-theme/down.svg" => 0xE74B,
        "windows-theme/download.svg" => 0xE896,
        "windows-theme/external-link.svg" => 0xE71B,
        "windows-theme/file.svg" => 0xE8A5,
        "windows-theme/folder.svg" => 0xE8B7,
        "windows-theme/folder-open.svg" => 0xE838,
        "windows-theme/forward.svg" => 0xE72A,
        "windows-theme/home.svg" => 0xE80F,
        "windows-theme/list.svg" => 0xE8FD,
        "windows-theme/menu.svg" => 0xE712,
        "windows-theme/monitor.svg" => 0xE7F4,
        "windows-theme/pin.svg" => 0xE718,
        "windows-theme/refresh.svg" => 0xE72C,
        "windows-theme/sort.svg" => 0xE8CB,
        "windows-theme/up.svg" => 0xE74A,
        "windows-theme/warning.svg" => 0xE7BA,
        "windows-theme/eye.svg" => 0xE890,
        _ => return None,
    };
    char::from_u32(glyph)
}

fn windows_theme_font_for_build(build: u32) -> &'static str {
    if build >= 22000 {
        "Segoe Fluent Icons"
    } else {
        "Segoe MDL2 Assets"
    }
}

#[cfg(target_os = "windows")]
pub fn windows_theme_font_family() -> &'static str {
    windows_theme_font_for_build(rf_win::osver::build())
}

#[cfg(target_os = "windows")]
pub fn windows_theme_glyph(glyph: char, size: Pixels) -> AnyElement {
    div()
        .size(size)
        .flex()
        .items_center()
        .justify_center()
        .font_family(windows_theme_font_family())
        .text_size(size)
        .child(glyph.to_string())
        .into_any_element()
}

pub fn windows_theme_icon_glyph(icon: L) -> Option<char> {
    let path = icon.path();
    let name = path.strip_prefix("icons/")?.strip_suffix(".svg")?;
    let glyph = match name {
        "arrow-down-up" | "arrow-up-down" => 0xE8CB,
        "chevron-down" => 0xE70D,
        "circle-plus" | "plus" => 0xE710,
        "clipboard-paste" => 0xE77F,
        "copy" => 0xE8C8,
        "ellipsis" => 0xE712,
        "eye" => 0xE890,
        "folder-input" | "folder-plus" | "folder-plus-2" => 0xE8F4,
        "layout-grid" => 0xECA5,
        "list-filter" | "filter" => 0xE71C,
        "panel-right" => 0xE890,
        "scissors" | "cut" => 0xE8C6,
        "share-2" => 0xE72D,
        "text-cursor-input" => 0xE8AC,
        "trash" | "trash-2" => 0xE74D,
        "wrench" => 0xE946,
        "rotate-cw" => 0xE72C,
        "rotate-ccw" => 0xE80D,
        "undo" | "undo-2" => 0xE7A7,
        "redo" | "redo-2" => 0xE7A6,
        _ => return None,
    };
    char::from_u32(glyph)
}

#[cfg(test)]
mod windows_theme_tests {
    use super::{Category, icon_for, windows_theme_context_glyph, windows_theme_icon_glyph, windows_theme_path};

    #[test]
    fn windows_theme_assets_cover_file_categories_and_common_explorer_actions() {
        for category in [
            Category::Folder,
            Category::Image,
            Category::Video,
            Category::Audio,
            Category::Archive,
            Category::Code,
            Category::Document,
            Category::Spreadsheet,
            Category::Pdf,
            Category::Executable,
            Category::Text,
            Category::Other,
        ] {
            let path = icon_for(category).path();
            assert!(
                windows_theme_path(&path).is_some(),
                "no Windows theme icon for {category:?}: {path}"
            );
        }
        for path in [
            "icons/arrow-left.svg",
            "icons/arrow-right.svg",
            "icons/folder-plus.svg",
            "icons/share-2.svg",
            "icons/usb.svg",
        ] {
            assert!(
                windows_theme_path(path).is_some(),
                "no Windows theme icon for {path}"
            );
        }
    }

    #[test]
    fn windows_theme_keeps_unmapped_glyphs_on_the_original_asset() {
        assert_eq!(windows_theme_path("icons/git-branch.svg"), None);
        assert!(!crate::settings::Settings::default().windows_file_explorer_theme);
    }

    #[test]
    fn windows_theme_does_not_change_the_meaning_of_unrelated_icons() {
        for path in [
            "icons/square.svg",
            "icons/user.svg",
            "icons/users.svg",
            "icons/send.svg",
            "icons/undo.svg",
            "icons/undo-2.svg",
            "icons/redo.svg",
            "icons/redo-2.svg",
            "icons/star.svg",
            "icons/bookmark.svg",
            "icons/info.svg",
            "icons/info-circle.svg",
            "icons/circle-help.svg",
            "icons/arrow-left-right.svg",
            "icons/settings.svg",
            "icons/search.svg",
            "icons/square-terminal.svg",
            "icons/settings-2.svg",
            "icons/sliders-horizontal.svg",
            "icons/cog.svg",
            "icons/wrench.svg",
        ] {
            assert_eq!(windows_theme_path(path), None, "{path}");
        }
    }

    #[test]
    fn windows_context_actions_use_segoe_fluent_glyphs() {
        for (asset, glyph) in [
            ("windows-theme/add.svg", '\u{E710}'),
            ("windows-theme/context-cut.svg", '\u{E8C6}'),
            ("windows-theme/context-copy.svg", '\u{E8C8}'),
            ("windows-theme/context-paste.svg", '\u{E77F}'),
            ("windows-theme/context-rename.svg", '\u{E8AC}'),
            ("windows-theme/context-share.svg", '\u{E72D}'),
            ("windows-theme/context-delete.svg", '\u{E74D}'),
            ("windows-theme/context-open-with.svg", '\u{E7AC}'),
            ("windows-theme/context-properties.svg", '\u{E946}'),
            ("windows-theme/context-shortcut.svg", '\u{E71B}'),
            ("windows-theme/context-new-folder.svg", '\u{E8F4}'),
            ("windows-theme/folder.svg", '\u{E8B7}'),
            ("windows-theme/home.svg", '\u{E80F}'),
        ] {
            assert_eq!(windows_theme_context_glyph(asset), Some(glyph), "{asset}");
        }
        assert_eq!(windows_theme_context_glyph("windows-theme/context-compress.svg"), None);
    }

    #[test]
    fn windows_theme_standard_actions_use_system_glyphs() {
        use gpui_kit::assets::IconName as L;

        assert_eq!(windows_theme_icon_glyph(L::Undo2), Some('\u{E7A7}'));
        assert_eq!(windows_theme_icon_glyph(L::Redo2), Some('\u{E7A6}'));
        assert_eq!(windows_theme_icon_glyph(L::Settings), None);
        assert_eq!(windows_theme_icon_glyph(L::Search), None);
        assert_eq!(windows_theme_icon_glyph(L::SquareTerminal), None);
    }

    #[test]
    fn windows_10_uses_mdl2_and_windows_11_uses_fluent_icons() {
        assert_eq!(super::windows_theme_font_for_build(19045), "Segoe MDL2 Assets");
        assert_eq!(super::windows_theme_font_for_build(22000), "Segoe Fluent Icons");
    }

    #[test]
    fn windows_context_rotation_and_new_folder_glyphs_are_distinct() {
        use gpui_kit::assets::IconName as L;

        assert_eq!(windows_theme_icon_glyph(L::RotateCw), Some('\u{E72C}'));
        assert_eq!(windows_theme_icon_glyph(L::RotateCcw), Some('\u{E80D}'));
        assert_eq!(windows_theme_icon_glyph(L::FolderInput), Some('\u{E8F4}'));
    }

    #[test]
    fn windows_toolbar_actions_use_fluent_glyphs() {
        use gpui_kit::assets::IconName as L;

        for (icon, glyph) in [
            (L::CirclePlus, '\u{E710}'),
            (L::Scissors, '\u{E8C6}'),
            (L::Copy, '\u{E8C8}'),
            (L::ClipboardPaste, '\u{E77F}'),
            (L::TextCursorInput, '\u{E8AC}'),
            (L::Share2, '\u{E72D}'),
            (L::Trash, '\u{E74D}'),
            (L::Wrench, '\u{E946}'),
            (L::Eye, '\u{E890}'),
            (L::RotateCw, '\u{E72C}'),
            (L::Ellipsis, '\u{E712}'),
            (L::ListFilter, '\u{E71C}'),
            (L::ArrowUpDown, '\u{E8CB}'),
            (L::LayoutGrid, '\u{ECA5}'),
            (L::PanelRight, '\u{E890}'),
            (L::ChevronDown, '\u{E70D}'),
        ] {
            assert_eq!(windows_theme_icon_glyph(icon), Some(glyph), "{}", icon.path());
        }
    }

    #[test]
    fn icon_selection_is_reversible_and_every_mapped_asset_is_embedded() {
        let icon = "icons/folder.svg";
        assert_eq!(super::resolved_windows_theme_path(icon, false), None);
        let selected = super::resolved_windows_theme_path(icon, true).unwrap();
        assert!(
            super::WINDOWS_THEME_ASSETS
                .iter()
                .any(|(path, _)| *path == selected)
        );

        for icon in gpui_kit::assets::IconName::ALL {
            if let Some(selected) = windows_theme_path(&icon.path()) {
                assert!(
                    super::WINDOWS_THEME_ASSETS
                        .iter()
                        .any(|(path, _)| *path == selected),
                    "missing asset for {}",
                    icon.path()
                );
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Folder,
    Image,
    Video,
    Audio,
    Archive,
    Code,
    Document,
    Spreadsheet,
    Pdf,
    Executable,
    Text,
    Other,
}

pub fn category(e: &Entry) -> Category {
    if e.is_dir() {
        return Category::Folder;
    }
    category_for_ext(&e.ext)
}

pub fn category_for_ext(ext: &str) -> Category {
    use Category::*;
    match ext {
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "tif" | "tiff" | "ico" | "heic" | "heif" | "avif"
        | "svg" | "psd" | "raw" | "cr2" | "cr3" | "nef" | "arw" | "dng" | "jfif" | "exr" => Image,
        "mp4" | "mov" | "mkv" | "avi" | "wmv" | "webm" | "m4v" | "mpg" | "mpeg" | "flv" | "mxf" | "braw"
        | "r3d" | "ts" | "m2ts" | "3gp" | "prproj" | "aep" | "drp" => Video,
        "mp3" | "wav" | "flac" | "aac" | "ogg" | "m4a" | "wma" | "opus" | "aiff" | "aif" | "mid" => Audio,
        "zip" | "7z" | "rar" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "zst" | "cab" | "iso" | "img" | "vhd"
        | "vhdx" => Archive,
        "rs" | "c" | "h" | "cpp" | "hpp" | "cs" | "js" | "jsx" | "ts_" | "tsx" | "py" | "go" | "java" | "kt"
        | "swift" | "rb" | "php" | "html" | "htm" | "css" | "scss" | "json" | "toml" | "yaml" | "yml"
        | "xml" | "sh" | "ps1" | "bat" | "cmd" | "lua" | "sql" | "vue" | "svelte" | "dart" | "zig" | "jsx_"
        | "mjs" | "cjs" | "ini" | "cfg" | "lock" => Code,
        "doc" | "docx" | "odt" | "rtf" | "pages" | "ppt" | "pptx" | "key" | "odp" => Document,
        "xls" | "xlsx" | "csv" | "ods" | "numbers" => Spreadsheet,
        "pdf" => Pdf,
        "exe" | "msi" | "appx" | "msix" | "dll" | "sys" | "lnk" | "url" | "com" | "scr" => Executable,
        "txt" | "md" | "log" | "nfo" | "srt" | "vtt" => Text,
        _ => Other,
    }
}

/// Files the shell can render a content thumbnail for.
pub fn has_thumbnail(e: &Entry) -> bool {
    if e.is_dir() || e.is_cloud_placeholder() {
        return false;
    }
    matches!(category(e), Category::Image | Category::Video | Category::Pdf)
        || matches!(&*e.ext, "docx" | "pptx" | "xlsx" | "psd" | "blend" | "3mf" | "stl" | "glb")
        || rf_win::design::is_design_ext(&e.ext)
}

pub fn icon_for(cat: Category) -> L {
    use Category::*;
    match cat {
        Folder => L::Folder,
        Image => L::FileImage,
        Video => L::FileVideoCamera,
        Audio => L::FileMusic,
        Archive => L::FileArchive,
        Code => L::FileCode,
        Document => L::FileText,
        Spreadsheet => L::FileSpreadsheet,
        Pdf => L::FileText,
        Executable => L::AppWindow,
        Text => L::FileText,
        Other => L::File,
    }
}

pub fn type_label(e: &Entry) -> String {
    type_label_of(e.is_dir(), &e.ext)
}

/// [`type_label`] for a folder or for a file's extension (lower case, no dot).
pub fn type_label_of(dir: bool, ext: &str) -> String {
    if dir {
        return t!("Dosya klasörü").into();
    }
    match ext {
        "" => t!("Dosya").into(),
        "exe" => t!("Uygulama").into(),
        "lnk" => t!("Kısayol").into(),
        "txt" => t!("Metin Belgesi").into(),
        "pdf" => t!("PDF Belgesi").into(),
        "zip" => t!("Sıkıştırılmış (zip) klasör").into(),
        ext => format!("{} {}", ext.to_uppercase(), t!("Dosyası")),
    }
}
