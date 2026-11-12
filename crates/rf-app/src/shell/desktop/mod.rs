//! The desktop view (shell plan increment F3): what the desktop will be, drawn in an ORDINARY resizable window that covers nothing.
//! `rustyfiles --desktop-preview` opens it; see [`args`] for the command line, including the scratch rule that lets it be tried
//! without touching the user's real Desktop.
//!
//! The view draws the model of `rf_core::desktop` / `rf_win::desktop` (F1, F2): the user's wallpaper placed for the window, and the
//! desktop's items on the layout store's grid. It is written to be hosted later by F4's per-monitor desktop windows: it takes its
//! items, monitors and layout file from a [`source::Backend`] and draws one monitor's CANVAS, which F4 shows 1:1 in a window that is
//! the monitor, and this preview shows scaled down when the window is smaller ([`geometry`]).
//!
//! | file | what it is | tested |
//! |---|---|---|
//! | `args` | the command line | table |
//! | `geometry` | window pixels <-> canvas pixels, the scale-down rule | table, 100 / 150 % |
//! | `tile` | where the picture and the label sit in a cell | table |
//! | `select` | click / Ctrl / Shift / rubber band, the arrows over a sparse grid | table |
//! | `typeahead` | type-to-select | table |
//! | `label` | two lines, the middle of the second goes | table |
//! | `label_shadow` | the shadow that keeps a name readable on any picture; its contrast arithmetic | table |
//! | `names` | the name under an icon | table |
//! | `rename_check` | what may be typed in the rename box | table |
//! | `drop` | what a drop means | table |
//! | `policy` | what may change files (read-only, Public Desktop, shortcuts to folders), when to ask before opening many | table |
//! | `choices` | the app's own choices for the system icons (Settings -> Masaüstü), carried to where the item source reads them | table |
//! | `menu_plan` | what the item menu and the empty-desktop menu hold for a selection and a place | table |
//! | `size_keys` | the chords that choose an icon size, and the forms a keyboard layout gives them | fake layouts |
//! | `menupoint` | which command may use the point of the last right click on the empty desktop | table |
//! | `links` | the names of the shortcuts the desktop makes | table |
//! | `source` | items, monitors and layout file; the scratch rule | temp folders |
//! | `wall` | the wallpaper bitmap | synthetic picture |
//! | `view`, `input`, `operations`, `actions`, `menus` | the GPUI view | not unit-testable (windows); run live (`actions` has the bin's job over a fake) |

pub mod args;
pub mod choices;
pub mod drop;
pub mod geometry;
pub mod label;
pub mod label_shadow;
pub mod links;
pub mod menu_plan;
pub mod menupoint;
pub mod size_keys;
pub mod names;
pub mod policy;
pub mod rename_check;
pub mod select;
pub mod shared;
pub mod source;
pub mod tile;
pub mod typeahead;
pub mod wall;

mod actions;
mod input;
mod menus;
pub mod open;
mod operations;
mod view;

pub use args::PreviewArgs;
pub use view::{DesktopView, Hosted};

use gpui_kit::*;

/// The key context of the view: the file list's own, so that every key the user has bound for the file list (and every override in
/// `settings.toml`) works on the desktop too, plus the desktop's own.
pub const CTX: &str = "FolderView DesktopView";

gpui_kit::actions!(rustyfiles_desktop, [ExtendLeft, ExtendRight, ExtendHome, ExtendEnd, IconsLarge, IconsMedium, IconsSmall]);

/// Keys of the desktop that the file list does not have: Shift+Left / Right extend the selection (the file list's Left and Right do
/// something else in its views, so it has no Shift form of them), and so do Shift+Home / End (to the first / last item). Ctrl+Shift+2,
/// 3 and 4 choose the large, medium and small icons, as in Explorer (the file list binds the same keys to its own views; the
/// desktop's context is the deeper one, so on the desktop these win).
fn bind(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("shift-left", ExtendLeft, Some("DesktopView")),
        KeyBinding::new("shift-right", ExtendRight, Some("DesktopView")),
        KeyBinding::new("shift-home", ExtendHome, Some("DesktopView")),
        KeyBinding::new("shift-end", ExtendEnd, Some("DesktopView")),
    ]);
    bind_sizes(cx);
}

fn size_action(size: rf_core::desktop::grid::IconSize) -> Box<dyn Action> {
    use rf_core::desktop::grid::IconSize;
    match size {
        IconSize::Large => Box::new(IconsLarge),
        IconSize::Medium => Box::new(IconsMedium),
        IconSize::Small => Box::new(IconsSmall),
    }
}

/// The three size keys: the layout-independent form (`size_keys::plain_keys`, the chords the menu's hints name) and the form the
/// keyboard layout produces (`size_keys::layout_keys`: GPUI reports Turkish Q's Ctrl+Shift+2 as `ctrl-"`, and Ctrl+Shift+4 as `ctrl-+`,
/// which the file list binds to zoom; on the desktop the menu says it is small icons, so it is bound here, see that module).
fn bind_sizes(cx: &mut App) {
    let Ok(context) = KeyBindingContextPredicate::parse("DesktopView") else { return };
    let context = std::rc::Rc::new(context);
    let wanted: Vec<(String, rf_core::desktop::grid::IconSize)> =
        size_keys::plain_keys().into_iter().map(|(k, s)| (k.to_string(), s)).chain(size_keys::layout_keys(rf_win::keys::shifted_char)).collect();
    let mut bindings = Vec::new();
    for (keys, size) in wanted {
        match KeyBinding::load(&keys, size_action(size), Some(context.clone()), false, None, cx) {
            Ok(b) => bindings.push(b),
            Err(e) => log::debug!("desktop: size key {keys}: {e}"),
        }
    }
    cx.bind_keys(bindings);
}

/// Binds the desktop's own keys. Once per process; the preview and the cover host both call it.
pub fn init(cx: &mut App) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| bind(cx));
}

/// The preview window. The process lives as long as it is open: the rustyfiles windows it opens (a double click on a folder) are
/// "main windows" and closing the last of them would otherwise end the whole process, preview included.
struct PreviewWindow(AnyWindowHandle);
impl Global for PreviewWindow {}

/// Whether the preview window is open.
pub fn window_open(cx: &App) -> bool {
    let open = cx.windows();
    cx.try_global::<PreviewWindow>().is_some_and(|g| open.iter().any(|w| w.window_id() == g.0.window_id()))
}

/// Opens the preview window. Nothing is read or listed on the calling thread: the window opens at once and fills in.
pub fn open_preview(args: PreviewArgs, cx: &mut App) {
    init(cx);
    let display = cx.primary_display().map(|d| d.bounds());
    // 90 % of the primary display, centred: close to the size of the canvas it shows by default (the primary monitor)
    let bounds = match display {
        Some(d) => {
            let (w, h) = (d.size.width * 0.9, d.size.height * 0.9);
            Bounds::new(point(d.origin.x + (d.size.width - w) / 2., d.origin.y + (d.size.height - h) / 2.), size(w, h))
        }
        None => Bounds::new(point(px(80.), px(80.)), size(px(1280.), px(800.))),
    };
    let title = if args.scratch() { crate::t!("rustyfiles masaüstü (önizleme, deneme klasörü)") } else { crate::t!("rustyfiles masaüstü (önizleme)") };
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions { title: Some(title.to_string().into()), ..Default::default() }),
        window_min_size: Some(size(px(480.), px(320.))),
        ..Default::default()
    };
    match gpui_kit::open_window(opts, cx, move |window, cx| cx.new(|cx| DesktopView::new(args, window, cx))) {
        Ok((handle, _)) => cx.set_global(PreviewWindow(handle)),
        Err(e) => log::error!("desktop preview: {e:#}"),
    }
}
