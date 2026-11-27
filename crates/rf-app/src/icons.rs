//! File categorization → symbol icon (Lucide) + type label.
//! Native shell icons/thumbnails come later via rf-win; this is the instant, zero-I/O fallback.

use crate::t;
use gpui_kit::assets::IconName as L;
#[cfg(not(target_os = "windows"))]
use gpui_kit::component::Icon;
use gpui_kit::{AnyElement, IntoElement, ParentElement, Pixels, Styled, div};
use rf_core::Entry;

pub fn windows_home_icon(size: Pixels) -> AnyElement {
    #[cfg(target_os = "windows")]
    {
        windows_theme_glyph('\u{E80F}', size)
    }
    #[cfg(not(target_os = "windows"))]
    {
        Icon::new(L::House).size(size).into_any_element()
    }
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
        "arrow-left" => 0xE72B,
        "arrow-right" => 0xE72A,
        "arrow-down-up" | "arrow-up-down" => 0xE8CB,
        "chevron-down" => 0xE70D,
        "circle-plus" | "plus" => 0xE710,
        "clipboard-copy" => 0xE8C8,
        "clipboard-paste" => 0xE77F,
        "copy" => 0xE8C8,
        "ellipsis" => 0xE712,
        "eye" => 0xE890,
        "folder-input" | "folder-plus" | "folder-plus-2" => 0xE8F4,
        "house" => 0xE80F,
        "layout-grid" => 0xECA5,
        "list-filter" | "filter" => 0xE71C,
        // (the preview pane: a box with a pane on its right, not the eye of "View", issue 400)
        "panel-right" => 0xEA4C,
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
    use super::windows_theme_icon_glyph;

    #[test]
    fn standard_actions_use_windows_glyphs_when_available() {
        use gpui_kit::assets::IconName as L;

        for (icon, glyph) in [
            (L::ArrowLeft, '\u{E72B}'),
            (L::ArrowRight, '\u{E72A}'),
            (L::ClipboardCopy, '\u{E8C8}'),
            (L::ClipboardPaste, '\u{E77F}'),
            (L::Copy, '\u{E8C8}'),
            (L::Eye, '\u{E890}'),
            (L::PanelRight, '\u{EA4C}'),
            (L::House, '\u{E80F}'),
            (L::Scissors, '\u{E8C6}'),
            (L::Share2, '\u{E72D}'),
            (L::TextCursorInput, '\u{E8AC}'),
            (L::Trash, '\u{E74D}'),
            (L::Wrench, '\u{E946}'),
        ] {
            assert_eq!(windows_theme_icon_glyph(icon), Some(glyph), "{}", icon.path());
        }
    }

    #[test]
    fn other_icons_keep_the_program_default() {
        use gpui_kit::assets::IconName as L;

        for icon in [
            L::Archive,
            L::ChartPie,
            L::Link,
            L::Search,
            L::Settings,
            L::SquareTerminal,
            L::Star,
        ] {
            assert_eq!(windows_theme_icon_glyph(icon), None, "{}", icon.path());
        }
    }

    #[test]
    fn windows_10_uses_mdl2_and_windows_11_uses_fluent_icons() {
        assert_eq!(super::windows_theme_font_for_build(19045), "Segoe MDL2 Assets");
        assert_eq!(super::windows_theme_font_for_build(22000), "Segoe Fluent Icons");
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

/// Every extension [`category_for_ext`] gives a kind (the same words, in the same order): what a search by kind asks for
/// (`search_kinds`). An extension added to the match below belongs here too; a test holds the two together one way.
pub const KNOWN_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "webp", "tif", "tiff", "ico", "heic", "heif", "avif", "svg", "psd", "raw", "cr2", "cr3",
    "nef", "arw", "dng", "jfif", "exr", "jxl", "jxr", "wdp", "mp4", "mov", "mkv", "avi", "wmv", "webm", "m4v", "mpg", "mpeg",
    "flv", "mxf", "braw", "r3d", "ts", "m2ts", "mts", "3gp", "prproj", "aep", "drp", "mp3", "wav", "flac", "aac", "ogg", "m4a", "wma",
    "opus", "aiff", "aif", "mid", "zip", "7z", "rar", "tar", "gz", "tgz", "bz2", "xz", "zst", "cab", "iso", "img", "vhd",
    "vhdx", "rs", "c", "h", "cpp", "hpp", "cs", "js", "jsx", "tsx", "py", "go", "java", "kt", "swift", "rb", "php", "html",
    "htm", "css", "scss", "json", "toml", "yaml", "yml", "xml", "sh", "ps1", "bat", "cmd", "lua", "sql", "vue", "svelte",
    "dart", "zig", "mjs", "cjs", "ini", "cfg", "lock", "doc", "docx", "odt", "rtf", "pages", "ppt", "pptx", "key", "odp", "xls",
    "xlsx", "csv", "ods", "numbers", "pdf", "exe", "msi", "appx", "msix", "dll", "sys", "lnk", "url", "com", "scr", "txt", "md",
    "log", "nfo", "srt", "vtt",
];

pub fn category_for_ext(ext: &str) -> Category {
    use Category::*;
    match ext {
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "tif" | "tiff" | "ico" | "heic" | "heif" | "avif"
        | "svg" | "psd" | "raw" | "cr2" | "cr3" | "nef" | "arw" | "dng" | "jfif" | "exr"
        // (JPEG XL, JPEG XR: pictured by the shell when a codec for them is installed, as jxl-winthumb is: issue 179)
        | "jxl" | "jxr" | "wdp" => Image,
        "mp4" | "mov" | "mkv" | "avi" | "wmv" | "webm" | "m4v" | "mpg" | "mpeg" | "flv" | "mxf" | "braw"
        | "r3d" | "ts" | "m2ts" | "mts" | "3gp" | "prproj" | "aep" | "drp" => Video,
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

/// Files the shell can render a content thumbnail for: the types of our own list, and those of `may_have_thumbnail`.
pub fn has_thumbnail(e: &Entry) -> bool {
    has_sure_thumbnail(e) || may_have_thumbnail(e)
}

/// The types that are pictured as a matter of course: pictures, videos, PDFs and a few more.
pub fn has_sure_thumbnail(e: &Entry) -> bool {
    if e.is_dir() || e.is_cloud_placeholder() {
        return false;
    }
    matches!(category(e), Category::Image | Category::Video | Category::Pdf)
        || matches!(&*e.ext, "docx" | "pptx" | "xlsx" | "psd" | "blend" | "3mf" | "stl" | "glb")
        || rf_win::design::is_design_ext(&e.ext)
}

/// Any other type a thumbnail provider is registered for with Windows: a sound file's cover, a font's sample, and whatever an
/// installed thumbnail extension draws (issue 307: SageThumbs 2K's types stayed icons). Such a file may or may not have a picture:
/// one that has none looks as it did before. Not programs and shortcuts: those keep their own icons (a shortcut's "thumbnail"
/// would be its target's).
pub fn may_have_thumbnail(e: &Entry) -> bool {
    !e.is_dir() && !e.is_cloud_placeholder() && !has_sure_thumbnail(e) && category(e) != Category::Executable && rf_win::thumbs::has_provider(&e.ext)
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
