//! Selectable file-icon packs. The default (`Windows`) asks the shell as before; a freedesktop pack
//! installed in `icon-themes/` overrides generic icons by extension or [`Category`] before the shell request is made.

use std::{collections::HashMap, path::{Path, PathBuf}, sync::{Arc, Mutex, OnceLock}};

use gpui_kit::RenderImage;

use crate::{icons::Category, settings::IconThemePref};

/// Rasterize SVGs at the size the row asks for: fixed-size artwork differs from `scalable`.
fn raster_px(requested: u32) -> u32 {
    requested.max(16)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DirKind {
    Fixed,
    Scalable,
    Threshold,
}

#[derive(Clone, Debug)]
struct ThemeDir {
    rel: String,
    size: u32,
    scale: u32,
    min_size: u32,
    max_size: u32,
    threshold: u32,
    kind: DirKind,
}

#[derive(Clone)]
struct Theme {
    inherits: Vec<String>,
    dirs: Vec<ThemeDir>,
}

fn themes_dir() -> Option<PathBuf> {
    crate::settings::app_dir().map(|d| d.join("icon-themes"))
}

/// Parsed packs are few and small; keep them instead of re-reading `index.theme` for every row.
static THEME_CACHE: OnceLock<Mutex<HashMap<String, Option<(PathBuf, Theme)>>>> = OnceLock::new();
/// Light/dark variant chosen for each selected pack and app appearance.
static EFFECTIVE_CACHE: OnceLock<Mutex<HashMap<(String, bool), String>>> = OnceLock::new();
/// Resolved asset path per pack, icon names and size. Without this every visible row repeats
/// dozens of `is_file` calls over the pack's directories.
static RESOLVED_CACHE: OnceLock<Mutex<HashMap<(String, String, u32), Option<PathBuf>>>> = OnceLock::new();

/// Folder names of installed freedesktop packs. A pack is a directory with an `index.theme`.
pub fn available_packs() -> Vec<String> {
    available_packs_in(themes_dir())
}

fn available_packs_in(root: Option<PathBuf>) -> Vec<String> {
    let Some(root) = root else { return Vec::new() };
    let mut out: Vec<String> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|e| e.path().join("index.theme").is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    out.sort();
    out
}

/// Installed variants of a theme family. Papirus ships each colour as a sibling pack
/// (`Papirus`, `Papirus-Dark`): the base pack is the family, the suffix is the variant. Returns
/// `(label, pack)` pairs, base first. Families without a second installed pack return one item.
pub fn theme_variants(pack: &str) -> Vec<(String, String)> {
    variants_from(&effective_pack(pack), &available_packs())
}

fn variants_from(pack: &str, packs: &[String]) -> Vec<(String, String)> {
    let base = pack
        .rsplit_once('-')
        .filter(|(base, suffix)| !base.is_empty() && !suffix.is_empty() && packs.iter().any(|p| p == base))
        .map(|(base, _)| base)
        .unwrap_or(pack);
    let mut out: Vec<(String, String)> = packs
        .iter()
        .filter(|p| p.as_str() == base || p.strip_prefix(base).is_some_and(|s| s.starts_with('-')))
        .map(|p| {
            let label = match p.strip_prefix(base).and_then(|s| s.strip_prefix('-')) {
                None | Some("") => format!("{base} ({})", crate::i18n::tx("varsayılan", "default")),
                Some(suffix) => {
                    let mut chars = suffix.chars();
                    match chars.next() {
                        Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
                        None => p.clone(),
                    }
                }
            };
            (label, p.clone())
        })
        .collect();
    out.sort_by(|a, b| (a.1 != base, &a.1).cmp(&(b.1 != base, &b.1)));
    if out.is_empty() {
        out.push((pack.to_string(), pack.to_string()));
    }
    out
}

/// Pick the family member that matches the app appearance. Packs that ship `Foo-Dark` and
/// `Foo-Light` follow Settings → Appearance; colour variants without a light/dark sibling stay put.
fn appearance_pack(pack: &str, dark: bool, packs: &[String]) -> String {
    let variants = variants_from(pack, packs);
    if variants.len() < 2 {
        return pack.to_string();
    }
    let base = variants[0].1.clone();
    let suffix = |p: &str| p.strip_prefix(&base).and_then(|s| s.strip_prefix('-')).unwrap_or("").to_ascii_lowercase();
    // A colour variant (`Papirus-Blue`) is a deliberate choice: keep it unless a matching dark sibling exists.
    if !matches!(suffix(pack).as_str(), "" | "light" | "dark" | "black") {
        return pack.to_string();
    }
    let members = if dark {
        variants.iter().find(|(_, p)| matches!(suffix(p).as_str(), "dark" | "black"))
    } else {
        variants.iter().find(|(_, p)| suffix(p) == "light")
    };
    members
        .or_else(|| variants.iter().find(|(_, p)| p == &base))
        .map(|(_, p)| p.clone())
        .unwrap_or_else(|| pack.to_string())
}

/// Resolve the selected pack through `appearance_pack` once per pack and appearance.
fn effective_pack(pack: &str) -> String {
    if pack.is_empty() {
        return String::new();
    }
    let cache = EFFECTIVE_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let key = (pack.to_owned(), crate::settings::effective_dark());
    if let Some(hit) = cache.lock().unwrap().get(&key) {
        return hit.clone();
    }
    let resolved = appearance_pack(pack, key.1, &available_packs());
    cache.lock().unwrap().insert(key, resolved.clone());
    resolved
}

/// A pack name is a single folder name, never a path: a setting from an old config cannot escape `icon-themes`.
fn pack_root(name: &str) -> Option<PathBuf> {
    pack_root_in(themes_dir(), name)
}

fn pack_root_in(root: Option<PathBuf>, name: &str) -> Option<PathBuf> {
    if name.is_empty() || name.contains(['/', '\\']) || name == "." || name == ".." {
        return None;
    }
    root.map(|d| d.join(name)).filter(|p| p.join("index.theme").is_file())
}

#[derive(Default)]
struct RawDirSection {
    size: Option<u32>,
    scale: Option<u32>,
    min_size: Option<u32>,
    max_size: Option<u32>,
    threshold: Option<u32>,
    kind: Option<DirKind>,
    context: String,
}

/// Symbolic subtrees and UI-chrome contexts (`actions`, `status`, `panel`, ...) hold monochrome toolbar
/// glyphs (`actions/16/directory.svg`, `places/symbolic/folder.svg`), not file-manager item icons.
fn is_item_icon_dir(rel: &str, context: &str) -> bool {
    if matches!(context, "actions" | "animations" | "status" | "emblems" | "emotes" | "panel" | "stock" | "intl") {
        return false;
    }
    for seg in rel.split(['/', '\\']) {
        let s = seg.to_ascii_lowercase();
        if s == "symbolic" || s.starts_with("symbolic-") || s.ends_with("-symbolic") {
            return false;
        }
        if matches!(s.as_str(), "actions" | "animations" | "status" | "emblems" | "emotes" | "panel") {
            return false;
        }
    }
    true
}

fn parse_index(root: &Path) -> Option<Theme> {
    let text = std::fs::read_to_string(root.join("index.theme")).ok()?;
    let mut section = String::new();
    let mut directories = Vec::new();
    let mut scaled_directories = Vec::new();
    let mut inherits = Vec::new();
    let mut sections: HashMap<String, RawDirSection> = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(['#', ';']) {
            continue;
        }
        if let Some(inner) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            section = inner.trim().to_ascii_lowercase();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let (key, value) = (key.trim().to_ascii_lowercase(), value.trim());
        if section == "icon theme" {
            match key.as_str() {
                "directories" => directories.extend(split_names(value)),
                "scaleddirectories" => scaled_directories.extend(split_names(value)),
                "inherits" => inherits = split_names(value),
                _ => {}
            }
        } else if !section.is_empty() {
            let entry = sections.entry(section.clone()).or_default();
            match key.as_str() {
                "size" => entry.size = value.parse().ok(),
                "scale" => entry.scale = value.parse().ok(),
                "minsize" => entry.min_size = value.parse().ok(),
                "maxsize" => entry.max_size = value.parse().ok(),
                "threshold" => entry.threshold = value.parse().ok(),
                "context" => entry.context = value.to_ascii_lowercase(),
                "type" => {
                    entry.kind = match value.to_ascii_lowercase().as_str() {
                        "fixed" => Some(DirKind::Fixed),
                        "scalable" => Some(DirKind::Scalable),
                        "threshold" => Some(DirKind::Threshold),
                        _ => None,
                    };
                }
                _ => {}
            }
        }
    }
    let mut seen = Vec::new();
    let dirs = directories
        .into_iter()
        .chain(scaled_directories)
        // `Directories` comes from a third-party pack: never let it point outside the pack folder.
        .filter(|rel| Path::new(rel).components().all(|c| matches!(c, std::path::Component::Normal(_))))
        .filter(|rel| {
            let low = rel.to_ascii_lowercase();
            if seen.contains(&low) {
                false
            } else {
                seen.push(low);
                true
            }
        })
        .filter_map(|rel| {
            let low = rel.to_ascii_lowercase();
            let raw = sections.get(&low);
            let context = raw.map_or("", |r| r.context.as_str());
            if !is_item_icon_dir(&rel, context) {
                return None;
            }
            let size = raw.and_then(|r| r.size).unwrap_or(0);
            let looks_scalable = rel.split(['/', '\\']).any(|s| s.to_ascii_lowercase().starts_with("scalable"));
            let kind = raw
                .and_then(|r| r.kind)
                .unwrap_or(if looks_scalable || size == 0 { DirKind::Scalable } else { DirKind::Threshold });
            let scale = raw.and_then(|r| r.scale).unwrap_or(1).max(1);
            let threshold = raw.and_then(|r| r.threshold).unwrap_or(2);
            let min_size = raw.and_then(|r| r.min_size).unwrap_or(if kind == DirKind::Scalable { 1 } else { size });
            let max_size = raw.and_then(|r| r.max_size).unwrap_or(if kind == DirKind::Scalable { u32::MAX } else { size });
            Some(ThemeDir { rel, size, scale, min_size, max_size, threshold, kind })
        })
        .collect();
    Some(Theme { inherits, dirs })
}

fn split_names(value: &str) -> Vec<String> {
    value.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect()
}

fn theme(pack: &str) -> Option<(PathBuf, Theme)> {
    let cache = THEME_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(hit) = cache.lock().unwrap().get(pack) {
        return hit.clone();
    }
    let found = pack_root(pack).and_then(|root| parse_index(&root).map(|t| (root, t)));
    cache.lock().unwrap().insert(pack.to_owned(), found.clone());
    found
}

fn dir_rank(d: &ThemeDir, px: u32) -> (u8, u32, u32) {
    let scale_tie = u32::from(d.scale != 1);
    // In Breeze/Papirus/KDE packs, fixed sub-24pt `places` (`16`, `16@2x`, `22`) hold monochrome panel glyphs;
    // when rendering >= 24px, prefer full-color fixed or scalable directories and keep < 24pt only as a last resort.
    if px >= 24 && d.kind != DirKind::Scalable && d.size > 0 && d.size < 24 {
        return (4, d.size.abs_diff(px), scale_tie);
    }
    match d.kind {
        DirKind::Fixed if d.size == px => (0, 0, scale_tie),
        DirKind::Threshold if d.size == px => (0, 0, scale_tie),
        DirKind::Threshold if px >= d.size.saturating_sub(d.threshold) && px <= d.size.saturating_add(d.threshold) => {
            (1, d.size.abs_diff(px), scale_tie)
        }
        DirKind::Scalable if px >= d.min_size && px <= d.max_size => {
            let nominal = if d.size == 0 { px } else { d.size };
            (2, nominal.abs_diff(px), scale_tie)
        }
        DirKind::Fixed | DirKind::Threshold => (3, d.size.abs_diff(px), scale_tie),
        DirKind::Scalable => {
            let dist = if px < d.min_size { d.min_size - px } else { px - d.max_size };
            (3, dist, scale_tie)
        }
    }
}

/// Freedesktop `FindBestIcon`: each candidate name is searched across ranked directories before falling back
/// to the next name, then across inherited themes.
fn find_icon(root: &Path, t: &Theme, names: &[&str], px: u32, visited: &mut Vec<String>) -> Option<PathBuf> {
    let mut dirs: Vec<&ThemeDir> = t.dirs.iter().collect();
    dirs.sort_by_key(|d| dir_rank(d, px));
    for name in names {
        for d in &dirs {
            for ext in ["svg", "png"] {
                let path = root.join(&d.rel).join(format!("{name}.{ext}"));
                if path.is_file() {
                    return Some(path);
                }
            }
        }
    }
    for parent in &t.inherits {
        if visited.iter().any(|v| v.eq_ignore_ascii_case(parent)) {
            continue;
        }
        visited.push(parent.clone());
        if let Some((next_root, next)) = theme(parent) {
            if let Some(path) = find_icon(&next_root, &next, names, px, visited) {
                return Some(path);
            }
        }
    }
    None
}

/// GPUI's image cache stores bytes as BGRA (see `gpui-pre/src/assets.rs` and `platform.rs`).
fn to_bgra(buf: &mut [u8]) {
    for pixel in buf.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
}

fn raster_svg(path: &Path, px: u32) -> Option<Arc<RenderImage>> {
    use resvg::{tiny_skia, usvg};
    static FONTS: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    let svg = std::fs::read(path).ok()?;
    let mut opt = usvg::Options::default();
    // A pack is user-installed third-party content: `<image href>` must not open local files.
    opt.image_href_resolver = usvg::ImageHrefResolver {
        resolve_data: usvg::ImageHrefResolver::default_data_resolver(),
        resolve_string: Box::new(|_, _| None),
    };
    opt.fontdb = FONTS
        .get_or_init(|| {
            let mut db = usvg::fontdb::Database::new();
            db.load_system_fonts();
            Arc::new(db)
        })
        .clone();
    let tree = usvg::Tree::from_data(&svg, &opt).ok()?;
    let px = raster_px(px);
    let scale = px as f32 / tree.size().width().max(tree.size().height()).max(1.);
    let mut pix = tiny_skia::Pixmap::new(px, px)?;
    resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pix.as_mut());
    // `Pixmap` stores premultiplied RGBA. GPUI expects straight alpha; passing premultiplied bytes darkens the icon edges.
    let mut bytes = pix.take();
    for pixel in bytes.chunks_exact_mut(4) {
        let a = pixel[3] as u16;
        if a == 0 {
            pixel[0] = 0;
            pixel[1] = 0;
            pixel[2] = 0;
        } else {
            for c in &mut pixel[..3] {
                *c = ((*c as u16 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    let mut buf = image::RgbaImage::from_raw(px, px, bytes)?;
    to_bgra(buf.as_mut());
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buf)])))
}

fn decode(path: &Path, px: u32) -> Option<Arc<RenderImage>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<Arc<RenderImage>>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let key = format!("{}:{px}", path.to_string_lossy());
    if let Some(hit) = cache.lock().unwrap().get(&key) {
        return hit.clone();
    }
    let img = if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("svg")) {
        raster_svg(path, px)
    } else {
        image::open(path).ok().map(|i| {
            let mut buf = i.to_rgba8();
            to_bgra(buf.as_mut());
            Arc::new(RenderImage::new(vec![image::Frame::new(buf)]))
        })
    };
    cache.lock().unwrap().insert(key, img.clone());
    img
}

/// Freedesktop icon name for a file kind. Extensions with a well-known name win; the category is the fallback.
fn freedesktop_name(cat: Category, ext: &str) -> &'static str {
    match ext {
        "pdf" => "application-pdf",
        "zip" => "application-zip",
        "rar" => "application-x-rar",
        "7z" => "application-x-7z-compressed",
        "tar" => "application-x-tar",
        "gz" | "tgz" => "application-x-compressed-tar",
        "xz" => "application-x-xz-compressed-tar",
        "bz2" => "application-x-bzip-compressed-tar",
        "zst" => "application-zstd",
        "iso" => "application-x-iso",
        "exe" | "msi" | "appx" | "msix" => "application-x-executable",
        "doc" | "docx" | "odt" | "rtf" => "x-office-document",
        "xls" | "xlsx" | "csv" | "ods" => "x-office-spreadsheet",
        "ppt" | "pptx" | "odp" => "x-office-presentation",
        "py" => "text-x-python",
        "rs" => "text-x-rust",
        "js" | "mjs" | "cjs" => "text-x-javascript",
        "html" | "htm" => "text-html",
        "css" => "text-css",
        "xml" => "text-xml",
        "json" => "application-json",
        "sh" | "bash" | "zsh" => "application-x-shellscript",
        "txt" | "md" | "log" | "nfo" => "text-x-generic",
        _ => match cat {
            Category::Folder => "folder",
            Category::Image => "image-x-generic",
            Category::Video => "video-x-generic",
            Category::Audio => "audio-x-generic",
            Category::Archive => "package-x-generic",
            Category::Executable => "application-x-executable",
            _ => "text-x-generic",
        },
    }
}

/// Freedesktop folder icon names, standard `folder` first, then MIME and legacy fallbacks.
pub const FOLDER_NAMES: &[&str] = &["folder", "inode-directory", "directory"];

fn freedesktop_image(pack: &str, cat: Category, ext: &str, px: u32) -> Option<Arc<RenderImage>> {
    let kind = freedesktop_name(cat, ext);
    let names = if cat == Category::Folder && ext.is_empty() { FOLDER_NAMES } else { std::slice::from_ref(&kind) };
    image_for_names(pack, names, px)
}

/// Keep the resolved path once per pack, icon-name list and size: the file walk is the expensive part.
fn cached_path(key: (String, String, u32), lookup: impl FnOnce() -> Option<PathBuf>) -> Option<PathBuf> {
    let cache = RESOLVED_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(hit) = cache.lock().unwrap().get(&key) {
        return hit.clone();
    }
    let found = lookup();
    cache.lock().unwrap().insert(key, found.clone());
    found
}

/// Icons used on the first paint of a folder, Home and the sidebar. Rasterizing them on a worker
/// before the first list render keeps the SVG parse and tiny-skia work off the UI thread.
const PREWARM_NAMES: &[&str] = &[
    "folder", "inode-directory", "directory", "user-desktop", "folder-desktop", "folder-downloads", "folder-download",
    "user-downloads", "folder-documents", "user-documents", "folder-pictures", "folder-picture",
    "user-pictures", "folder-music", "user-music", "folder-video", "folder-videos", "user-videos",
    "user-home", "folder-home", "drive-harddisk", "drive-harddisk-system", "drive-removable-media",
    "drive-harddisk-usb", "drive-optical", "drive-cdrom", "network-server", "network-workgroup",
    "folder-network", "folder-remote", "folder-cloud", "utilities-terminal", "terminal", "user-trash",
    "text-x-generic", "image-x-generic", "video-x-generic", "audio-x-generic", "package-x-generic",
    "application-x-executable", "application-pdf", "application-zip", "application-x-rar",
    "application-x-7z-compressed", "application-x-tar", "application-x-compressed-tar",
    "application-x-xz-compressed-tar", "application-x-bzip-compressed-tar", "application-zstd",
    "application-x-iso", "x-office-document", "x-office-spreadsheet", "x-office-presentation",
    "text-x-python", "text-x-rust", "text-x-javascript", "text-html", "text-css", "text-xml",
    "application-json", "application-x-shellscript",
];

/// Resolve and rasterize the common icons. Called on a background executor; the decode cache is
/// shared with the render path, so a hit there needs no SVG work on the UI thread.
pub fn prewarm(pack: &str) {
    if pack.is_empty() {
        return;
    }
    for name in PREWARM_NAMES {
        for px in [crate::image_cache::ICON_PX, 64] {
            let _ = image_for_names(pack, &[name], px);
        }
    }
}

/// Look up explicit freedesktop icon names, first match wins. Used for locations (drives, WSL,
/// Downloads) where a file kind is too generic.
pub fn image_for_names(pack: &str, names: &[&str], px: u32) -> Option<Arc<RenderImage>> {
    let pack = effective_pack(pack);
    let key = (pack.clone(), names.join("\0"), px);
    let path = cached_path(key, || {
        let (root, t) = theme(&pack)?;
        let mut visited = vec![pack.clone()];
        find_icon(&root, &t, names, px, &mut visited)
    })?;
    decode(&path, px)
}

fn norm_path_key(path: &Path) -> String {
    let mut s = path.to_string_lossy().replace('/', "\\").to_lowercase();
    while s.len() > 3 && s.ends_with('\\') {
        s.pop();
    }
    s
}

/// True when `parent` is a user profile root (`C:\Users\alice`) or a OneDrive root (`H:\OneDrive`,
/// `C:\Users\alice\OneDrive - Org`). Leaf-name fallbacks only apply directly under these roots so
/// ordinary project subfolders (`repo\docs`, `Downloads\Pictures`) stay plain folders.
fn is_special_parent_root(parent: &Path, roots: &[String]) -> bool {
    let key = norm_path_key(parent);
    if roots.iter().any(|r| r == &key) {
        return true;
    }
    // `<drive>:\users\<name>` or `<drive>:\users\<name>\onedrive...`
    let b = key.as_bytes();
    if b.len() > 9 && b[1] == b':' && b[2] == b'\\' {
        if let Some(rest) = key[3..].strip_prefix("users\\") {
            return match rest.split_once('\\') {
                None => !rest.is_empty(),
                Some((user, sub)) => !user.is_empty() && !sub.contains('\\') && sub.starts_with("onedrive"),
            };
        }
    }
    false
}

/// Theme icon names for Windows' known folders. Exact known-folder paths match first; localized
/// leaf names only match direct children of a profile or OneDrive root, never nested subfolders.
pub fn special_folder_names(path: &Path) -> &'static [&'static str] {
    static KNOWN: OnceLock<(Vec<(String, &'static [&'static str])>, Vec<String>)> = OnceLock::new();
    let (known, roots) = KNOWN.get_or_init(|| {
        use rf_win::known::KnownFolder;
        let all = [
            KnownFolder::Desktop,
            KnownFolder::Downloads,
            KnownFolder::Documents,
            KnownFolder::Pictures,
            KnownFolder::Music,
            KnownFolder::Videos,
            KnownFolder::Profile,
        ];
        let known = all
            .iter()
            .filter_map(|k| k.path().map(|p| (norm_path_key(&p), known_folder_names(*k))))
            .collect();
        let mut roots = Vec::new();
        if let Some(p) = KnownFolder::Profile.path() {
            roots.push(norm_path_key(&p));
        }
        for var in ["USERPROFILE", "OneDrive", "OneDriveConsumer", "OneDriveCommercial"] {
            if let Ok(val) = std::env::var(var) {
                if !val.trim().is_empty() {
                    roots.push(norm_path_key(Path::new(val.trim())));
                }
            }
        }
        (known, roots)
    });
    let full = norm_path_key(path);
    if let Some((_, names)) = known.iter().find(|(p, _)| full == *p) {
        return names;
    }
    let Some(parent) = path.parent() else { return &[] };
    if !is_special_parent_root(parent, roots) {
        return &[];
    }
    match path.file_name().and_then(|n| n.to_str()).map(|n| n.to_lowercase()).as_deref() {
        Some("desktop" | "área de trabalho") => &["user-desktop", "folder-desktop"],
        Some("downloads" | "transferências") => &["folder-downloads", "folder-download", "user-downloads"],
        Some("documents" | "documentos") => &["folder-documents", "user-documents"],
        Some("pictures" | "imagens") => &["folder-pictures", "folder-picture", "user-pictures"],
        Some("music" | "música" | "musicas" | "músicas") => &["folder-music", "user-music"],
        Some("videos" | "vídeos") => &["folder-video", "folder-videos", "user-videos"],
        _ => &[],
    }
}

fn known_folder_names(kind: rf_win::known::KnownFolder) -> &'static [&'static str] {
    use rf_win::known::KnownFolder;
    match kind {
        KnownFolder::Desktop => &["user-desktop", "folder-desktop"],
        KnownFolder::Downloads => &["folder-downloads", "folder-download", "user-downloads"],
        KnownFolder::Documents => &["folder-documents", "user-documents"],
        KnownFolder::Pictures => &["folder-pictures", "folder-picture", "user-pictures"],
        KnownFolder::Music => &["folder-music", "user-music"],
        KnownFolder::Videos => &["folder-video", "folder-videos", "user-videos"],
        KnownFolder::Profile => &["user-home", "folder-home"],
    }
}

/// A themed picture, or `None` for `Windows` and for anything the pack does not cover.
pub fn image(theme: IconThemePref, pack: &str, cat: Category, ext: &str, px: u32) -> Option<Arc<RenderImage>> {
    match theme {
        IconThemePref::Windows | IconThemePref::Legacy => None,
        IconThemePref::Freedesktop => freedesktop_image(pack, cat, ext, px),
    }
}

/// Current setting, for callers outside the icon cache (Home, sidebar) that only need a picture.
pub fn picture_for_names(names: &[&str], px: u32) -> Option<Arc<RenderImage>> {
    match crate::settings::icon_theme() {
        IconThemePref::Freedesktop => image_for_names(&crate::settings::icon_theme_pack(), names, px),
        _ => None,
    }
}

/// Current setting, for callers that have a file kind instead of an icon name.
pub fn picture_for_kind(cat: Category, ext: &str, px: u32) -> Option<Arc<RenderImage>> {
    match crate::settings::icon_theme() {
        IconThemePref::Freedesktop => freedesktop_image(&crate::settings::icon_theme_pack(), cat, ext, px),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    static SEQ: AtomicU32 = AtomicU32::new(0);

    #[test]
    fn a_prewarmed_picture_can_cross_threads() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Arc<gpui_kit::RenderImage>>();
        assert_send_sync::<Mutex<HashMap<(String, String, u32), Option<PathBuf>>>>();
    }

    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("rustyfiles-icon-{tag}-{}-{}", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn default_keeps_shell_icons() {
        assert!(image(IconThemePref::Windows, "", Category::Folder, "", 16).is_none());
    }

    #[test]
    fn freedesktop_names_cover_plain_kinds() {
        assert_eq!(freedesktop_name(Category::Folder, ""), "folder");
        assert_eq!(freedesktop_name(Category::Image, "jpg"), "image-x-generic");
        assert_eq!(freedesktop_name(Category::Other, "pdf"), "application-pdf");
        assert_eq!(freedesktop_name(Category::Code, "py"), "text-x-python");
    }

    #[test]
    fn index_theme_reads_directories_and_inheritance() {
        let root = temp_root("index");
        std::fs::write(root.join("index.theme"), "[Icon Theme]\nName=Test\nInherits=hicolor\nDirectories=16x16/mimetypes,scalable/mimetypes\n\n[16x16/mimetypes]\nSize=16\nContext=mimetypes\n\n[scalable/mimetypes]\nContext=mimetypes\n").unwrap();
        let t = parse_index(&root).unwrap();
        assert_eq!(t.inherits, ["hicolor"]);
        assert_eq!(t.dirs.len(), 2);
        assert_eq!(t.dirs[0].size, 16);
        assert_eq!(t.dirs[1].size, 0);
        assert_eq!(t.dirs[1].rel, "scalable/mimetypes");
        std::fs::remove_dir_all(root).ok();
    }

    fn fixed_dir(rel: &str, size: u32) -> ThemeDir {
        ThemeDir { rel: rel.into(), size, scale: 1, min_size: size, max_size: size, threshold: 2, kind: DirKind::Fixed }
    }

    fn scalable_dir(rel: &str, size: u32) -> ThemeDir {
        ThemeDir { rel: rel.into(), size, scale: 1, min_size: 1, max_size: u32::MAX, threshold: 2, kind: DirKind::Scalable }
    }

    #[test]
    fn exact_size_wins_at_its_size_and_scalable_wins_at_large_sizes() {
        let root = temp_root("lookup");
        let small = root.join("16x16/mimetypes");
        let big = root.join("scalable/mimetypes");
        std::fs::create_dir_all(&small).unwrap();
        std::fs::create_dir_all(&big).unwrap();
        std::fs::write(small.join("text-x-generic.png"), b"png").unwrap();
        std::fs::write(big.join("text-x-generic.svg"), b"<svg/>").unwrap();
        let t = Theme { inherits: Vec::new(), dirs: vec![fixed_dir("16x16/mimetypes", 16), scalable_dir("scalable/mimetypes", 0)] };
        let names = vec!["text-x-generic"];
        let found = find_icon(&root, &t, &names, 16, &mut Vec::new()).unwrap();
        assert_eq!(found, small.join("text-x-generic.png"));
        let found = find_icon(&root, &t, &names, 64, &mut Vec::new()).unwrap();
        assert_eq!(found, big.join("text-x-generic.svg"), "scalable artwork beats a 16px panel icon at 64px");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn scalable_used_when_no_fixed_size_has_the_icon() {
        let root = temp_root("lookup-scalable");
        let small = root.join("16x16/mimetypes");
        let big = root.join("scalable/mimetypes");
        std::fs::create_dir_all(&small).unwrap();
        std::fs::create_dir_all(&big).unwrap();
        std::fs::write(big.join("text-x-generic.svg"), b"<svg/>").unwrap();
        let t = Theme { inherits: Vec::new(), dirs: vec![fixed_dir("16x16/mimetypes", 16), scalable_dir("scalable/mimetypes", 0)] };
        let names = vec!["text-x-generic"];
        let found = find_icon(&root, &t, &names, 16, &mut Vec::new()).unwrap();
        assert_eq!(found, big.join("text-x-generic.svg"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn svg_packs_rasterize_to_a_square_picture() {
        let root = temp_root("svg");
        let path = root.join("folder.svg");
        std::fs::write(&path, "<svg xmlns='http://www.w3.org/2000/svg' width='24' height='24'><rect width='24' height='24' fill='red'/></svg>").unwrap();
        let img = decode(&path, 16).unwrap();
        assert_eq!(img.as_bytes(0).unwrap().len(), (raster_px(16) * raster_px(16) * 4) as usize);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn svg_raster_keeps_straight_alpha() {
        let root = temp_root("svg-alpha");
        let path = root.join("half.svg");
        std::fs::write(&path, "<svg xmlns='http://www.w3.org/2000/svg' width='24' height='24'><rect width='24' height='24' fill='#ffffff' fill-opacity='0.5'/></svg>").unwrap();
        let img = decode(&path, 16).unwrap();
        let bytes = img.as_bytes(0).unwrap();
        let px = ((8 * raster_px(16) + 8) * 4) as usize;
        assert!(bytes[px] > 200, "white at 50% must stay white, got {}", bytes[px]);
        assert!((bytes[px + 3] as i32 - 128).abs() < 16, "alpha should stay ~128, got {}", bytes[px + 3]);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn svg_raster_is_bgra_for_gpui() {
        let root = temp_root("svg-bgra");
        let path = root.join("blue.svg");
        std::fs::write(&path, "<svg xmlns='http://www.w3.org/2000/svg' width='16' height='16'><rect width='16' height='16' fill='#3366cc'/></svg>").unwrap();
        let img = decode(&path, 16).unwrap();
        let bytes = img.as_bytes(0).unwrap();
        assert_eq!(&bytes[..4], &[0xcc, 0x66, 0x33, 0xff]);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn old_xp_setting_keeps_shell_icons() {
        assert!(image(IconThemePref::Legacy, "", Category::Folder, "", 16).is_none());
    }

    #[test]
    fn a_resolved_icon_path_is_only_looked_up_once() {
        let key = (format!("lookup-cache-{}", SEQ.fetch_add(1, Ordering::Relaxed)), "folder\0directory".into(), 16);
        let path = PathBuf::from(r"C:\theme\folder.svg");
        let calls = Cell::new(0);
        let first = cached_path(key.clone(), || {
            calls.set(calls.get() + 1);
            Some(path.clone())
        });
        let second = cached_path(key, || {
            calls.set(calls.get() + 1);
            None
        });
        assert_eq!(first.as_deref(), Some(path.as_path()));
        assert_eq!(second, first);
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn dark_and_light_variants_follow_the_app() {
        let packs: Vec<String> = ["Papirus", "Papirus-Dark", "Papirus-Light", "Papirus-Blue", "KDE-Modern-Classic"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(appearance_pack("Papirus", true, &packs), "Papirus-Dark");
        assert_eq!(appearance_pack("Papirus-Dark", false, &packs), "Papirus-Light");
        assert_eq!(appearance_pack("Papirus-Light", true, &packs), "Papirus-Dark");
        assert_eq!(appearance_pack("Papirus-Blue", true, &packs), "Papirus-Blue");
        assert_eq!(appearance_pack("KDE-Modern-Classic", true, &packs), "KDE-Modern-Classic");
    }

    #[test]
    fn variants_group_sibling_packs() {
        let packs: Vec<String> = ["Papirus", "Papirus-Dark", "Papirus-Light", "KDE-Modern-Classic"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let variants = variants_from("Papirus-Dark", &packs);
        let names: Vec<&str> = variants.iter().map(|(_, p)| p.as_str()).collect();
        assert_eq!(names, ["Papirus", "Papirus-Dark", "Papirus-Light"]);
        // without the base pack a suffix is just part of the name, not a variant
        let single = variants_from("KDE-Modern-Classic", &packs);
        assert_eq!(single.len(), 1);
    }

    #[test]
    fn special_folders_get_their_own_icon_names_without_leaking_to_subfolders() {
        assert_eq!(special_folder_names(Path::new(r"C:\Users\x\Downloads"))[0], "folder-downloads");
        assert_eq!(special_folder_names(Path::new(r"C:\Users\x\Documents"))[0], "folder-documents");
        assert_eq!(special_folder_names(Path::new(r"C:\Users\x\OneDrive\Pictures"))[0], "folder-pictures");
        assert!(special_folder_names(Path::new(r"C:\Users\x\Downloads\Subfolder")).is_empty(), "subfolders inside Downloads stay plain folders");
        assert!(special_folder_names(Path::new(r"C:\Users\x\Downloads\Pictures")).is_empty(), "a nested Pictures inside Downloads stays a plain folder");
        assert!(special_folder_names(Path::new(r"H:\Workspace\repo\Documents")).is_empty(), "project folders named Documents stay plain folders");
        assert!(special_folder_names(Path::new(r"C:\Users\x\Random")).is_empty());
    }

    #[test]
    fn accented_special_folder_names_fall_back_to_the_leaf() {
        assert_eq!(special_folder_names(Path::new(r"C:\Users\x\Imagens"))[0], "folder-pictures");
        assert_eq!(special_folder_names(Path::new(r"C:\Users\x\Música"))[0], "folder-music");
        assert_eq!(special_folder_names(Path::new(r"C:\Users\x\Vídeos"))[0], "folder-video");
        assert!(special_folder_names(Path::new(r"C:\Users\x\Imagens\Férias")).is_empty());
    }

    #[test]
    fn pack_names_are_single_folder_names() {
        let root = temp_root("pack-name");
        std::fs::create_dir_all(root.join("Valid")).unwrap();
        std::fs::write(root.join("Valid/index.theme"), "[Icon Theme]\nName=Valid\n").unwrap();
        assert_eq!(pack_root_in(Some(root.clone()), "Valid"), Some(root.join("Valid")));
        for bad in ["", ".", "..", "Valid/Sub", r"Valid\Sub", "/abs", r"C:\abs", "Missing"] {
            assert_eq!(pack_root_in(Some(root.clone()), bad), None, "{bad} must not be a pack");
        }
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn only_folders_with_an_index_theme_are_packs() {
        let root = temp_root("packs");
        for name in ["A", "B", "C"] {
            std::fs::create_dir_all(root.join(name)).unwrap();
        }
        std::fs::write(root.join("A/index.theme"), "[Icon Theme]\nName=A\n").unwrap();
        std::fs::write(root.join("C/index.theme"), "[Icon Theme]\nName=C\n").unwrap();
        std::fs::write(root.join("loose.txt"), b"x").unwrap();
        assert_eq!(available_packs_in(Some(root.clone())), ["A", "C"]);
        assert!(available_packs_in(None).is_empty());
        assert!(available_packs_in(Some(root.join("Missing"))).is_empty());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn index_theme_ignores_comments_and_case_but_skips_escaping_directories() {
        let root = temp_root("index-guard");
        std::fs::write(root.join("index.theme"), "; comment\n[ICON THEME]\n# another\nNAME=Bad\nINHERITS=hicolor\nDIRECTORIES=good,../escape,/abs,C:\\outside\n\n[GOOD]\nSIZE=24\n").unwrap();
        let t = parse_index(&root).unwrap();
        assert_eq!(t.inherits, ["hicolor"]);
        assert_eq!(t.dirs.len(), 1, "only the in-pack directory survives");
        assert_eq!(t.dirs[0].rel, "good");
        assert_eq!(t.dirs[0].size, 24);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn missing_index_theme_is_not_a_pack() {
        let root = temp_root("index-missing");
        assert!(parse_index(&root).is_none());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn icon_names_are_tried_in_order_and_svg_beats_png() {
        let root = temp_root("name-order");
        let dir = root.join("32x32/places");
        let fallback_dir = root.join("32x32/mimetypes");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(&fallback_dir).unwrap();
        std::fs::write(dir.join("folder.svg"), b"<svg/>").unwrap();
        std::fs::write(dir.join("folder.png"), b"png").unwrap();
        std::fs::write(fallback_dir.join("inode-directory.svg"), b"<svg/>").unwrap();
        let t = Theme { inherits: Vec::new(), dirs: vec![fixed_dir("32x32/mimetypes", 32), fixed_dir("32x32/places", 32)] };
        let found = find_icon(&root, &t, FOLDER_NAMES, 32, &mut Vec::new()).unwrap();
        assert_eq!(found, dir.join("folder.svg"), "`folder` in places beats `inode-directory` in an earlier mimetypes dir, and svg beats png");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn actions_symbolic_and_panel_dirs_never_hijack_folder_icons() {
        let root = temp_root("kora-papirus");
        for rel in ["16x16/actions", "16x16/places", "places/symbolic", "places/scalable"] {
            std::fs::create_dir_all(root.join(rel)).unwrap();
        }
        std::fs::write(root.join("16x16/actions/directory.svg"), b"<svg/>").unwrap();
        std::fs::write(root.join("16x16/places/folder.svg"), b"<svg/>").unwrap();
        std::fs::write(root.join("places/symbolic/folder.svg"), b"<svg/>").unwrap();
        std::fs::write(root.join("places/scalable/folder.svg"), b"<svg/>").unwrap();
        std::fs::write(
            root.join("index.theme"),
            "[Icon Theme]\nName=Test\nDirectories=16x16/actions,16x16/places,places/symbolic,places/scalable\n\n\
             [16x16/actions]\nSize=16\nContext=Actions\nType=Fixed\n\n\
             [16x16/places]\nSize=16\nContext=Places\nType=Fixed\n\n\
             [places/symbolic]\nSize=16\nContext=Places\nType=Scalable\nMinSize=8\nMaxSize=512\n\n\
             [places/scalable]\nSize=64\nContext=Places\nType=Scalable\nMinSize=8\nMaxSize=512\n",
        )
        .unwrap();
        let t = parse_index(&root).unwrap();
        let found = find_icon(&root, &t, FOLDER_NAMES, 32, &mut Vec::new()).unwrap();
        assert_eq!(found, root.join("places/scalable/folder.svg"), "32px folder lookup skips actions, symbolic and 16pt panel glyphs");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn inheritance_cycles_do_not_hang_the_lookup() {
        let root = temp_root("cycle");
        let t = Theme { inherits: vec!["self".into()], dirs: Vec::new() };
        let found = find_icon(&root, &t, &["folder"], 16, &mut vec!["self".into()]);
        assert_eq!(found, None, "the visited pack is not opened again");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn svg_external_images_are_not_loaded() {
        let root = temp_root("svg-href");
        let mut red = image::RgbaImage::new(4, 4);
        for p in red.pixels_mut() {
            *p = image::Rgba([255, 0, 0, 255]);
        }
        let png = root.join("red.png");
        red.save(&png).unwrap();
        let href = format!("file:///{}", png.to_string_lossy().replace('\\', "/"));
        let svg = format!("<svg xmlns='http://www.w3.org/2000/svg' xmlns:xlink='http://www.w3.org/1999/xlink' width='16' height='16'><image xlink:href='{href}' width='16' height='16'/></svg>");
        let path = root.join("external.svg");
        std::fs::write(&path, svg).unwrap();
        let bytes = decode(&path, 16).unwrap().as_bytes(0).unwrap().to_vec();
        // decode swaps to BGRA, so a loaded red pixel would be [0, 0, 255, 255].
        assert_ne!(&bytes[..4], &[0, 0, 255, 255], "SVG must not read files from the pack's host");
        assert_eq!(bytes[3], 0, "the unresolved image draws nothing");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn decode_caches_missing_files() {
        let key = (format!("decode-miss-{}", SEQ.fetch_add(1, Ordering::Relaxed)), "missing".into(), 16);
        let path = PathBuf::from(format!(r"C:\rustyfiles-no-such-icon-{}.png", key.0));
        let calls = Cell::new(0);
        let lookup = || {
            calls.set(calls.get() + 1);
            path.exists().then_some(path.clone())
        };
        let first = cached_path(key.clone(), lookup);
        let second = cached_path(key, lookup);
        assert_eq!(first, None);
        assert_eq!(second, first);
        assert_eq!(calls.get(), 1, "a miss must be cached too");
    }

    #[test]
    fn raster_size_is_clamped() {
        assert_eq!(raster_px(8), 16);
        assert_eq!(raster_px(24), 24);
        assert_eq!(raster_px(512), 512, "large requests keep their size");
    }

    #[test]
    fn extension_aliases_share_their_icon_names() {
        for ext in ["tgz", "gz"] {
            assert_eq!(freedesktop_name(Category::Archive, ext), "application-x-compressed-tar");
        }
        for ext in ["doc", "docx", "odt", "rtf"] {
            assert_eq!(freedesktop_name(Category::Other, ext), "x-office-document");
        }
        assert_eq!(freedesktop_name(Category::Other, "zst"), "application-zstd");
        assert_eq!(freedesktop_name(Category::Executable, "msix"), "application-x-executable");
        assert_eq!(freedesktop_name(Category::Video, "mkv"), "video-x-generic");
    }
}
