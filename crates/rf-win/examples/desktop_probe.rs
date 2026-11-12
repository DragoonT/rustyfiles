//! Windowless, read-only probe of the desktop model (shell plan F1 and F2). Prints: the monitors; the wallpaper (where it was read from,
//! position mode, background, and per monitor the file TYPE, never the path); the desktop items by source and which virtual items are
//! on; a dry run of the layout rules on an in-memory store; and whether the import of Explorer's icon positions works (how many, the
//! range of the coordinates, how well they fit the monitors under each scale). No names of the user's files or paths are printed.
//! Nothing is written (no registry, no setting, no file), no window is created or shown.
//!
//!   cargo run -p rf-win --example desktop_probe              (about a second)
//!   cargo run -p rf-win --example desktop_probe -- --render  (also decodes the wallpaper in memory and times it; writes nothing)

use std::sync::Arc;
use std::time::Instant;

use rf_core::desktop::grid::{Grid, IconSize, metrics};
use rf_core::desktop::import::{Assumption, ImportFailure, coordinate_range, convert, diagnose};
use rf_core::desktop::items::{DesktopModel, FolderOrigin, ItemId, Origin, Visibility, VirtualState, sort_items};
use rf_core::desktop::layout::MonitorSpec;
use rf_core::desktop::placement::{TileAnchor, plan};
use rf_core::desktop::registry::{Hive, RegRead};
use rf_core::desktop::store::Store;
use rf_core::desktop::virtuals::{self, VirtualKind, Why};
use rf_core::desktop::wallpaper::{Source, is_transcoded_copy};
use rf_core::desktop::{Rect, Size};
use rf_core::{SortDir, SortKey};
use rf_win::desktop::items::{Folders, SystemFolders, public_desktop_dir, user_desktop_dir};
use rf_win::desktop::registry::SystemRegistry;
use rf_win::desktop::render::{Decoder, ImageDecoder, RenderOptions, render_all};
use rf_win::desktop::wallpaper::{DiskStamper, current, file_type};
use rf_win::desktop::{import, monitors};

fn r(x: &Rect) -> String {
    format!("({}, {}, {}, {})", x.left, x.top, x.right, x.bottom)
}

fn main() {
    let render = std::env::args().any(|a| a == "--render");
    let aware = monitors::make_process_per_monitor_aware();
    let ms = monitors::list();
    println!("== MONITORS: {} (per-monitor aware set by this probe: {aware})", ms.len());
    for (i, m) in ms.iter().enumerate() {
        println!(
            "  #{i}{} rect={} {}x{} work={} scale={}% dpi={} device-key-is-interface-path={}",
            if m.primary { " primary" } else { "" },
            r(&m.rect),
            m.rect.width(),
            m.rect.height(),
            r(&m.work),
            m.scale_percent,
            m.dpi,
            m.device_key.starts_with(r"\\?\DISPLAY#")
        );
    }
    let specs: Vec<MonitorSpec> = ms.iter().map(|m| m.spec()).collect();

    // ---- F1: the wallpaper
    let reg: Arc<SystemRegistry> = Arc::new(SystemRegistry);
    let t = Instant::now();
    let wp = current(reg.clone(), &monitors::device_rects(&ms));
    println!("\n== WALLPAPER (read in {:?})", t.elapsed());
    println!("  source: {}", if wp.source == Source::DesktopWallpaperCom { "IDesktopWallpaper (COM)" } else { "registry fallback (COM gave nothing)" });
    println!("  position mode: {}{}", wp.position.name(), if wp.position_assumed { " (ASSUMED: unknown style number)" } else { "" });
    println!("  kind: {:?}  background colour: {:?}  slideshow: {:?}", wp.kind, wp.background, wp.slideshow);
    println!("  per-virtual-desktop values: {} desktops, current has its own picture: {}", wp.per_desktop.len(), wp.per_desktop.iter().any(|d| d.current && d.picture.is_some()));
    let pictures = wp.pictures();
    for (i, m) in wp.monitors.iter().enumerate() {
        let same_key = ms.iter().find(|x| x.rect == m.rect).map(|x| x.device_key == m.device);
        let (ty, dims) = match &pictures[i] {
            Some(p) => (file_type(p).unwrap_or("not a picture type we know"), rf_win::look::decode::dimensions(p)),
            None => ("(no picture: colour)", None),
        };
        let transcoded = pictures[i].as_deref().is_some_and(is_transcoded_copy);
        println!(
            "  monitor {i}: rect={} file type={ty} transcoded-copy={transcoded} pixels={:?} (COM device path equals our key: {})",
            r(&m.rect),
            dims,
            if wp.source == Source::DesktopWallpaperCom { format!("{same_key:?}") } else { "n/a".into() }
        );
    }
    let rects: Vec<Rect> = wp.monitors.iter().map(|m| m.rect).collect();
    let vs = rf_core::desktop::geom::virtual_screen(&rects);
    if let Some((w, h)) = pictures.iter().flatten().next().and_then(|p| rf_win::look::decode::dimensions(p)) {
        println!("  placement of a {w} x {h} picture ({}):", wp.position.name());
        for (i, m) in wp.monitors.iter().enumerate() {
            let p = plan(wp.position, Size::new(w, h), m.rect, vs, TileAnchor::VirtualScreen);
            println!("    monitor {i}: use {} of the picture -> draw at {} in a {}x{} bitmap, {} bar(s), tile origin {:?}", r(&p.src), r(&p.dst), p.target.w, p.target.h, p.bars.len(), p.tile);
        }
    }
    if render {
        match pictures.iter().flatten().next() {
            Some(p) => {
                let t = Instant::now();
                let dec = ImageDecoder.decode(p);
                println!("  decode of the first picture: {:?} -> {}", t.elapsed(), dec.as_ref().map(|b| format!("{}x{}", b.width, b.height)).unwrap_or_else(|e| format!("failed: {e:#}")));
                drop(dec);
                let t = Instant::now();
                let out = render_all(&wp, &RenderOptions { anchor: TileAnchor::VirtualScreen, cache: None }, &ImageDecoder, &DiskStamper);
                println!("  decode + one bitmap per monitor, no cache: {:?} -> {}", t.elapsed(), out.iter().map(|b| format!("{}x{}{}", b.bitmap.width, b.bitmap.height, b.note.as_ref().map(|n| format!(" [{n}]")).unwrap_or_default())).collect::<Vec<_>>().join(", "));
            }
            None => println!("  --render: no picture to decode"),
        }
    }

    // ---- F2: the items
    println!("\n== DESKTOP ITEMS");
    let (user_dir, public_dir) = (user_desktop_dir(), public_desktop_dir());
    println!(
        "  known folders: user found={} public found={} different={} user redirected to a cloud folder={}",
        user_dir.is_some(),
        public_dir.is_some(),
        user_dir != public_dir,
        user_dir.as_ref().is_some_and(|p| p.to_string_lossy().to_lowercase().contains("onedrive"))
    );
    let folders = SystemFolders::new();
    let mut model = DesktopModel::new(Visibility::default());
    for o in [FolderOrigin::User, FolderOrigin::Public] {
        match folders.list(o) {
            Ok(files) => {
                let hidden = files.iter().filter(|f| f.attrs & 2 != 0).count();
                let system = files.iter().filter(|f| f.attrs & 4 != 0).count();
                let dirs = files.iter().filter(|f| f.is_dir).count();
                let cloud = files.iter().filter(|f| f.attrs & (0x1000 | 0x40000 | 0x400000) != 0).count();
                println!("  {o:?}: {} entries ({dirs} folders, {hidden} hidden, {system} system, {cloud} online-only placeholders)", files.len());
                model.set_files(o, files);
            }
            Err(e) => println!("  {o:?}: could not list ({e})"),
        }
    }
    let vset = virtuals::read(reg.as_ref());
    println!("  virtual items enabled: {}", if vset.shown().is_empty() { "none".to_string() } else { vset.shown().iter().map(|k| format!("{} ({})", k.english_name(), if matches!(vset.kinds.iter().find(|(kk, _)| kk == k).unwrap().1, Why::Set(_)) { "set by the user" } else { "default" })).collect::<Vec<_>>().join(", ") });
    println!("  not shown: {}", VirtualKind::ALL.iter().filter(|k| !vset.is_shown(**k)).map(|k| k.english_name()).collect::<Vec<_>>().join(", "));
    println!("  show-desktop-icons switch: {}  NoDesktop policy: {}", vset.show_icons, vset.policy_no_desktop);
    println!("  Explorer's show-hidden / super-hidden: Hidden={:?} ShowSuperHidden={:?}", reg.dword(Hive::CurrentUser, virtuals::KEY_ADVANCED, "Hidden"), reg.dword(Hive::CurrentUser, virtuals::KEY_ADVANCED, "ShowSuperHidden"));
    let recycle_full = rf_win::sta::call(std::time::Duration::from_secs(5), || rf_win::recycle::info().map(|(n, _)| n > 0).unwrap_or(false)).unwrap_or(false);
    println!("  Recycle Bin has items: {recycle_full}");
    let vstate = VirtualState { shown: vset.shown(), recycle_full, disabled: vset.desktop_disabled() };
    model.set_virtuals(vstate);
    let count = |m: &DesktopModel| {
        let it = m.items();
        let by = |f: fn(&Origin) -> bool| it.iter().filter(|i| f(&i.origin)).count();
        (it.len(), by(|o| matches!(o, Origin::Virtual(_))), by(|o| matches!(o, Origin::User)), by(|o| matches!(o, Origin::Public)), it.iter().filter(|i| i.same_name_elsewhere).count())
    };
    let c = count(&model);
    println!("  merged, hidden and system files hidden: {} items = {} virtual + {} user + {} public; names in both folders (items): {}", c.0, c.1, c.2, c.3, c.4);
    model.set_visibility(Visibility { show_hidden: true, show_system: true, dot_hidden: false });
    let c2 = count(&model);
    println!("  merged, show hidden + system on:        {} items = {} virtual + {} user + {} public", c2.0, c2.1, c2.2, c2.3);
    model.set_visibility(Visibility::default());
    // the shell's own count, for the T11 comparison: enumerating the desktop folder object (no hidden items unless the user's setting shows them)
    let shell_count = rf_win::sta::call(std::time::Duration::from_secs(10), || {
        for name in ["shell:::{00021400-0000-0000-C000-000000000046}", "shell:Desktop", "::{00021400-0000-0000-C000-000000000046}"] {
            let mut n = 0usize;
            if rf_win::shellfolder::list(name, &|| false, &mut |b| n += b.len()).is_ok() {
                return Some((n, name));
            }
        }
        None
    })
    .flatten();
    println!(
        "  the shell's own enumeration of the Desktop folder object: {}",
        shell_count.map(|(n, via)| format!("{n} items (asked as \"{via}\"; hidden ones only if Explorer's setting shows them)")).unwrap_or_else(|| "not available".into())
    );

    // ---- the layout rules on an in-memory store (never saved)
    println!("\n== LAYOUT (dry run, in memory)");
    let mut items = model.items();
    sort_items(&mut items, SortKey::Name, SortDir::Asc);
    let ids: Vec<ItemId> = items.iter().map(|i| i.id.clone()).collect();
    let mut store = Store::default();
    for s in &specs {
        let g = Grid::new(s.work, metrics(IconSize::Medium, s.scale_percent));
        println!("  grid of {}: {} x {} cells of {} px (medium icons at {}%)", r(&s.rect), g.cols, g.rows, g.metrics.pitch_x, s.scale_percent);
    }
    let res = store.resolve(&specs, rf_core::desktop::layout::Listing::complete(&ids), 0);
    let per: Vec<usize> = (0..specs.len()).map(|i| res.placed.values().filter(|p| p.monitor == i).count()).collect();
    println!("  {} items -> placed {} (per monitor {per:?}), unplaced {}, fresh {}", ids.len(), res.placed.len(), res.unplaced.len(), res.fresh.len());

    // ---- the import of Explorer's positions
    println!("\n== IMPORT OF EXPLORER'S ICON POSITIONS (IFolderView::GetItemPosition, read-only)");
    let t = Instant::now();
    match import::read_explorer_positions(user_dir.clone(), public_dir.clone()) {
        Ok(ep) => {
            println!("  worked in {:?}: {} items in Explorer's view, {} positions read and named, {} not ours (profile folder, OneDrive, libraries ...)", t.elapsed(), ep.total, ep.positions.len(), ep.unmapped);
            let ours: std::collections::BTreeSet<ItemId> = model.items().into_iter().map(|i| i.id).collect();
            let theirs: std::collections::BTreeSet<ItemId> = ep.positions.iter().map(|p| p.id.clone()).collect();
            println!(
                "  our item list against Explorer's view: {} in both, {} only in ours, {} only in Explorer's (the default show-hidden rules)",
                ours.intersection(&theirs).count(),
                ours.difference(&theirs).count(),
                theirs.difference(&ours).count()
            );
            if let Some(((x0, x1), (y0, y1))) = coordinate_range(&ep.positions) {
                println!("  coordinates as returned: x {x0} .. {x1}, y {y0} .. {y1} (virtual screen {} x {}, origin {}, {})", vs.width(), vs.height(), vs.left, vs.top);
            }
            println!("  how many land on a monitor's grid if the numbers are in units of N % pixels: {:?}", diagnose(&ep.positions, &specs, IconSize::Medium));
            match convert(&ep.positions, &specs, IconSize::Medium, Assumption::default()) {
                Ok(c) => println!("  at face value (physical pixels, origin = virtual screen's top-left): accepted, {} of {} fit", c.fit, c.total),
                Err(ImportFailure::UnitsDoubtful { fit, total }) => println!("  at face value: REFUSED, only {fit} of {total} fit (the units are not what the importer assumes: shell plan test T13)"),
                Err(e) => println!("  at face value: {e}"),
            }
        }
        Err(e) => println!("  could not import: {e} (after {:?})", t.elapsed()),
    }
}
