//! Which picture a dock icon is made from and at which sizes it is kept. Pure: sizes, file names and frame lists are arguments, so
//! every decision is tested without a window, a disk or the shell.
//!
//! WHY THIS EXISTS. An icon of the dock is drawn at 44 dock units at rest and up to 1.55 times that under the pointer, in device
//! pixels `44 * peak * (monitor scale * dock size)`. A picture is sharp only when it has at least that many pixels of its OWN detail,
//! so the questions are: how many pixels are needed ([`peak_px`]), which stored picture to start from ([`choose_asset`] for a packaged
//! app's logo files, [`choose_frame`] for the frames of a program's icon), and which smaller copies to keep so that the picture drawn
//! at any moment of the grow is never reduced by more than a small step ([`level_sizes`], [`pick_level`]).

use super::layout::ICON_REST;

/// The smallest and the largest picture the dock keeps for one icon (device px on a side).
pub const MIN_PX: u32 = 16;
pub const MAX_PX: u32 = 512;

/// The most a kept picture is reduced by when it is drawn (a ratio of its size to the size it is drawn at). Between two kept sizes the
/// drawn size moves by at most this much, so the toolkit's plain bilinear sampling never skips more than a fraction of a source pixel
/// (which is what makes a picture drawn much smaller look soft or shimmer). At the crossing between two kept sizes the picture
/// swaps and its sharpness takes a step of up to this much: how visible that is while the icons grow was not seen on a screen.
pub const MAX_STEP: f32 = 1.3;

/// A scale that cannot be right (zero, negative, not a number) reads as 1.
fn sane(v: f32) -> f32 {
    if v.is_finite() && v > 0.0 { v } else { 1.0 }
}

/// Device px of an icon at rest. `unit` is device px per dock unit (the monitor's scale times the dock's size).
pub fn rest_px(unit: f32) -> u32 {
    ((ICON_REST * sane(unit)).round() as u32).clamp(MIN_PX, MAX_PX)
}

/// The most device px an icon is ever drawn at: its rest size times the magnification's peak (1 when the magnification is off), times
/// the unit. Never less than [`rest_px`].
pub fn peak_px(unit: f32, peak: f32) -> u32 {
    let peak = if peak.is_finite() { peak.max(1.0) } else { 1.0 };
    ((ICON_REST * peak * sane(unit)).ceil() as u32).clamp(rest_px(unit), MAX_PX)
}

/// The sizes of the pictures kept for one icon, smallest first: the rest size, the peak size and, between them, as many sizes as it
/// takes to keep every step below [`MAX_STEP`] (geometric, like a mip chain). One size when the icon never grows.
pub fn level_sizes(rest: u32, peak: u32) -> Vec<u32> {
    let rest = rest.clamp(MIN_PX, MAX_PX);
    let peak = peak.clamp(rest, MAX_PX);
    if peak == rest {
        return vec![rest];
    }
    let ratio = peak as f32 / rest as f32;
    let steps = ((ratio.ln() / MAX_STEP.ln()).ceil().max(1.0) as u32).min(5);
    let mut sizes: Vec<u32> = (0..=steps).map(|i| (rest as f32 * ratio.powf(i as f32 / steps as f32)).round() as u32).collect();
    sizes[0] = rest;
    sizes[steps as usize] = peak;
    sizes.dedup();
    sizes
}

/// The kept picture to draw an icon that is `drawn` device px wide: the smallest that is not smaller than it (it is reduced, never
/// enlarged); the largest when the icon is drawn bigger than all of them. A fraction of a pixel under a size still takes that size
/// (enlarging by less than a pixel cannot be seen, and the pictures would otherwise swap at every rounding). `sizes` is ascending.
pub fn pick_level(sizes: &[u32], drawn: f32) -> usize {
    sizes.iter().position(|&s| s as f32 + 0.75 >= drawn).unwrap_or(sizes.len().saturating_sub(1))
}

// ---- a packaged app's logo files

/// One logo file of a package, as far as its name tells: how many pixels it has on a side, and whether it is the "unplated" form
/// (the glyph on a transparent ground, which is what the taskbar draws; the plain form carries a coloured plate).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetVariant {
    pub px: u32,
    pub unplated: bool,
}

/// What a file name in a package's asset folder says about the logo `stem` (the manifest's `Assets\Square44x44Logo`, here
/// `Square44x44Logo`) whose plain size is `base_px`. The names are `stem.png`, `stem.scale-200.png`, `stem.targetsize-256.png` and
/// `stem.targetsize-48_altform-unplated.png`. A file made for another purpose (high contrast, a theme, a language, the light taskbar's
/// glyph, a DirectX feature level) is not a candidate: `None`.
pub fn parse_asset(file_name: &str, stem: &str, ext: &str, base_px: u32) -> Option<AssetVariant> {
    let lower = file_name.to_ascii_lowercase();
    let stem = stem.to_ascii_lowercase();
    let ext = format!(".{}", ext.to_ascii_lowercase());
    let middle = lower.strip_prefix(stem.as_str())?.strip_suffix(ext.as_str())?;
    if middle.is_empty() {
        return Some(AssetVariant { px: base_px, unplated: false });
    }
    let (mut scale, mut target, mut unplated) = (None, None, false);
    for q in middle.strip_prefix('.')?.split('_') {
        let (key, value) = q.split_once('-')?;
        match (key, value) {
            ("scale", v) => scale = Some(v.parse::<u32>().ok().filter(|s| (1..=1000).contains(s))?),
            ("targetsize", v) => target = Some(v.parse::<u32>().ok().filter(|s| (1..=1024).contains(s))?),
            ("altform", "unplated") => unplated = true,
            ("contrast", "standard") => {}
            _ => return None,
        }
    }
    let px = match (target, scale) {
        (Some(t), _) => t,
        (None, Some(s)) => (base_px * s).div_ceil(100),
        (None, None) => base_px,
    };
    Some(AssetVariant { px, unplated })
}

/// How many px the unplated files of a package must reach for the dock to take the unplated form: what a package made for the
/// taskbar ships (256) is far above it, and it covers every size the dock draws at 200 % with the default dock size. A fixed number,
/// not the size needed now, so that the form (a glyph on a transparent ground, or the coloured plate) does not change when the dock's
/// size, the monitor's scale or the magnification does.
pub const UNPLATED_ENOUGH_PX: u32 = 128;

/// The best of a package's logo files for an icon that needs `need` device px: among the unplated ones when there are any and they
/// reach [`UNPLATED_ENOUGH_PX`] (or the plated ones are no bigger), else among the plated ones; there the smallest that has `need` px
/// or more, else the biggest there is. The form depends on the files only, never on `need`. The index into `v`.
pub fn choose_asset(v: &[AssetVariant], need: u32) -> Option<usize> {
    let best_of = |pool: &[usize]| -> Option<usize> {
        let enough = pool.iter().copied().filter(|&i| v[i].px >= need).min_by_key(|&i| v[i].px);
        enough.or_else(|| pool.iter().copied().max_by_key(|&i| v[i].px))
    };
    let all: Vec<usize> = (0..v.len()).collect();
    let unplated: Vec<usize> = all.iter().copied().filter(|&i| v[i].unplated).collect();
    let top = |pool: &[usize]| pool.iter().map(|&i| v[i].px).max().unwrap_or(0);
    if !unplated.is_empty() && (top(&unplated) >= UNPLATED_ENOUGH_PX || top(&unplated) >= top(&all)) {
        return best_of(&unplated);
    }
    let plated: Vec<usize> = all.into_iter().filter(|&i| !v[i].unplated).collect();
    best_of(&plated)
}

/// The newest of the installed versions of a package family, by the version in the full name (`Name_1.2.3.4_x64__publisher`). The
/// full names are what `GetPackagesByPackageFamily` lists; one that does not parse is older than any that does.
pub fn newest_package(full_names: &[String]) -> Option<usize> {
    let version = |n: &str| -> Option<[u32; 4]> {
        let v = n.split('_').nth(1)?;
        let mut parts = v.split('.').map(|p| p.parse::<u32>().ok());
        Some([parts.next()??, parts.next()??, parts.next()??, parts.next()??])
    };
    (0..full_names.len()).max_by_key(|&i| version(&full_names[i]))
}

/// The logo a package's manifest names for an application: the path as written (`Assets\Square44x44Logo.png`) and the size the name
/// stands for (44 for `Square44x44Logo`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManifestLogo {
    pub relative: String,
    pub base_px: u32,
}

/// The logo of application `app_id` (the part of the AppUserModelID after the `!`) in the text of an `AppxManifest.xml`: the
/// taskbar-sized one (`Square44x44Logo`), else the 30 px one older packages have, else the tile's. An application the manifest does not
/// list by that id falls back to the first one it lists.
pub fn manifest_logo(xml: &str, app_id: &str) -> Option<ManifestLogo> {
    let doc = roxmltree::Document::parse_with_options(xml, roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() }).ok()?;
    let apps: Vec<_> = doc.descendants().filter(|n| n.is_element() && n.tag_name().name() == "Application").collect();
    let app = apps.iter().find(|a| a.attribute("Id").is_some_and(|id| id.eq_ignore_ascii_case(app_id))).or(apps.first())?;
    let visual = app.descendants().find(|n| n.is_element() && n.tag_name().name() == "VisualElements")?;
    [("Square44x44Logo", 44), ("Square30x30Logo", 30), ("Square150x150Logo", 150)]
        .into_iter()
        .find_map(|(attr, base_px)| visual.attribute(attr).filter(|v| !v.trim().is_empty()).map(|v| ManifestLogo { relative: v.trim().to_string(), base_px }))
}

/// A manifest's logo path split into its folder, its name without extension and its extension (`Assets\Square44x44Logo.png`).
pub fn split_logo_path(relative: &str) -> Option<(String, String, String)> {
    let (dir, file) = relative.rsplit_once(['\\', '/']).unwrap_or(("", relative));
    let (stem, ext) = file.rsplit_once('.')?;
    (!stem.is_empty() && !ext.is_empty()).then(|| (dir.to_string(), stem.to_string(), ext.to_string()))
}

// ---- the frames of a program's icon

/// One frame of an icon group as the group directory lists it (`width` and `height` are 0 for 256).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameInfo {
    pub width: u32,
    pub height: u32,
    pub bpp: u16,
}

impl FrameInfo {
    /// The frame's side in px (the directory writes 256 as 0).
    pub fn side(self) -> u32 {
        let s = self.width.max(self.height);
        if s == 0 { 256 } else { s }
    }
}

/// The frame to start from for an icon that needs `need` px: the smallest that has `need` or more, else the largest; among frames of
/// the same size the one with the most colour depth. The index into `frames`.
pub fn choose_frame(frames: &[FrameInfo], need: u32) -> Option<usize> {
    let key = |i: usize| (frames[i].side(), frames[i].bpp);
    let enough = (0..frames.len()).filter(|&i| frames[i].side() >= need).min_by_key(|&i| (frames[i].side(), std::cmp::Reverse(frames[i].bpp)));
    enough.or_else(|| (0..frames.len()).max_by_key(|&i| key(i)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dock::magnify::{MAX_PEAK, Strength};

    #[test]
    fn the_size_needed_is_rest_times_peak_times_the_unit() {
        // 44 x 1.55 = 68.2 -> 69 at 100 %
        assert_eq!(peak_px(1.0, MAX_PEAK), 69);
        // 125 %, 150 %, 200 % of the monitor at the default dock size
        assert_eq!(peak_px(1.25, MAX_PEAK), 86);
        assert_eq!(peak_px(1.5, MAX_PEAK), 103);
        assert_eq!(peak_px(2.0, MAX_PEAK), 137);
        // the dock's size multiplies it: Küçük 0.85, Büyük 1.2, Çok büyük 1.4 (on a 150 % monitor the unit is the product)
        assert_eq!(peak_px(0.85, MAX_PEAK), 58);
        assert_eq!(peak_px(1.2, MAX_PEAK), 82);
        assert_eq!(peak_px(1.4 * 1.5, MAX_PEAK), 144);
        // the subtle setting and no magnification at all need less
        assert_eq!(peak_px(1.0, Strength::Subtle.peak()), 55);
        assert_eq!(peak_px(1.0, Strength::Off.peak()), 44);
        // nonsense reads as the default, and the answer stays in range
        assert_eq!(peak_px(0.0, MAX_PEAK), 69);
        assert_eq!(peak_px(f32::NAN, f32::NAN), 44);
        assert_eq!(peak_px(100.0, MAX_PEAK), MAX_PX);
        assert_eq!(rest_px(1.0), 44);
        assert_eq!(rest_px(1.25), 55);
        assert_eq!(rest_px(0.01), MIN_PX);
    }

    #[test]
    fn the_peak_is_never_smaller_than_the_rest_size() {
        for unit in [0.5f32, 0.85, 1.0, 1.1, 1.25, 1.4, 1.5, 1.75, 2.0, 2.5, 3.0, 4.2] {
            for peak in [0.5f32, 1.0, 1.25, 1.55] {
                assert!(peak_px(unit, peak) >= rest_px(unit), "unit {unit} peak {peak}");
                assert!(peak_px(unit, peak) as f32 + 1.0 >= ICON_REST * peak.max(1.0) * unit || peak_px(unit, peak) == MAX_PX);
            }
        }
    }

    #[test]
    fn the_kept_sizes_step_down_by_no_more_than_the_limit() {
        assert_eq!(level_sizes(44, 44), vec![44]);
        assert_eq!(level_sizes(44, 69), vec![44, 55, 69]);
        assert_eq!(level_sizes(44, 55), vec![44, 55]);
        assert_eq!(level_sizes(88, 137), vec![88, 110, 137]);
        for unit in [0.85f32, 1.0, 1.25, 1.5, 2.0, 2.8, 3.0] {
            for peak in [1.0f32, 1.25, 1.55] {
                let sizes = level_sizes(rest_px(unit), peak_px(unit, peak));
                assert_eq!(sizes[0], rest_px(unit));
                assert_eq!(*sizes.last().unwrap(), peak_px(unit, peak));
                for pair in sizes.windows(2) {
                    assert!(pair[1] > pair[0], "ascending, no duplicates: {sizes:?}");
                    assert!(pair[1] as f32 / pair[0] as f32 <= MAX_STEP + 0.03, "{sizes:?} at {unit} x {peak}");
                }
            }
        }
        // a wrong order or an absurd request cannot make a long list
        assert_eq!(level_sizes(500, 20), vec![500]);
        assert!(level_sizes(1, 100000).len() <= 6, "a absurd range stays a short list");
    }

    #[test]
    fn a_drawn_size_takes_the_smallest_kept_picture_that_is_not_smaller() {
        let sizes = [44, 55, 69];
        assert_eq!(pick_level(&sizes, 44.0), 0);
        assert_eq!(pick_level(&sizes, 30.0), 0, "the smallest, reduced");
        assert_eq!(pick_level(&sizes, 44.6), 0, "less than a pixel of enlarging is not worth swapping for");
        assert_eq!(pick_level(&sizes, 46.0), 1);
        assert_eq!(pick_level(&sizes, 55.0), 1);
        assert_eq!(pick_level(&sizes, 60.0), 2);
        assert_eq!(pick_level(&sizes, 68.2), 2);
        assert_eq!(pick_level(&sizes, 500.0), 2, "bigger than all: the biggest");
        assert_eq!(pick_level(&[44], 68.0), 0);
        assert_eq!(pick_level(&[], 68.0), 0);
    }

    #[test]
    fn logo_file_names_say_their_size_and_whether_they_are_unplated() {
        let p = |n: &str| parse_asset(n, "Square44x44Logo", "png", 44);
        let v = |px, unplated| Some(AssetVariant { px, unplated });
        assert_eq!(p("Square44x44Logo.png"), v(44, false));
        assert_eq!(p("Square44x44Logo.scale-100.png"), v(44, false));
        assert_eq!(p("Square44x44Logo.scale-200.png"), v(88, false));
        assert_eq!(p("Square44x44Logo.scale-400.png"), v(176, false));
        assert_eq!(p("Square44x44Logo.scale-125.png"), v(55, false));
        assert_eq!(p("Square44x44Logo.targetsize-256.png"), v(256, false));
        assert_eq!(p("Square44x44Logo.targetsize-48_altform-unplated.png"), v(48, true));
        assert_eq!(p("Square44x44Logo.altform-unplated_targetsize-256.png"), v(256, true));
        assert_eq!(p("SQUARE44X44LOGO.TARGETSIZE-24_ALTFORM-UNPLATED.PNG"), v(24, true), "a file system does not care about the case");
        assert_eq!(p("Square44x44Logo.scale-200_contrast-standard.png"), v(88, false));
        // not candidates: other purposes, other logos, other file kinds
        assert_eq!(p("Square44x44Logo.targetsize-48_altform-lightunplated.png"), None);
        assert_eq!(p("Square44x44Logo.scale-200_contrast-white.png"), None);
        assert_eq!(p("Square44x44Logo.scale-200_theme-light.png"), None);
        assert_eq!(p("Square44x44Logo.scale-200_language-en-us.png"), None);
        assert_eq!(p("Square150x150Logo.scale-200.png"), None);
        assert_eq!(p("Square44x44Logo.scale-200.jpg"), None);
        assert_eq!(p("Square44x44Logo.scale-abc.png"), None);
        assert_eq!(p("Square44x44Logo.scale-0.png"), None);
        assert_eq!(p("Square44x44Logo2.png"), None);
        assert_eq!(p("Square44x44Logo..png"), None);
        // a logo of another base size
        assert_eq!(parse_asset("Square150x150Logo.scale-200.png", "Square150x150Logo", "png", 150), v(300, false));
    }

    fn av(px: u32, unplated: bool) -> AssetVariant {
        AssetVariant { px, unplated }
    }

    #[test]
    fn the_logo_file_is_the_smallest_that_is_big_enough_and_unplated_when_there_is_one() {
        // a typical Store app: plated scales to 176, unplated target sizes to 256
        let v = [av(44, false), av(55, false), av(66, false), av(88, false), av(176, false), av(16, true), av(24, true), av(32, true), av(48, true), av(256, true)];
        assert_eq!(choose_asset(&v, 69), Some(9), "unplated 256 is the only unplated one with 69 px: reduced from it");
        assert_eq!(choose_asset(&v, 40), Some(8), "unplated 48");
        assert_eq!(choose_asset(&v, 500), Some(9), "nothing is big enough: the biggest");
        // no unplated files: the smallest plated one that is big enough
        let plated = [av(44, false), av(55, false), av(88, false), av(176, false)];
        assert_eq!(choose_asset(&plated, 69), Some(2));
        assert_eq!(choose_asset(&plated, 44), Some(0));
        assert_eq!(choose_asset(&plated, 200), Some(3));
        // the unplated files stop short and a plated file is bigger: sharpness wins over the missing plate
        let short = [av(16, true), av(32, true), av(44, false), av(176, false)];
        assert_eq!(choose_asset(&short, 103), Some(3));
        assert_eq!(choose_asset(&short, 20), Some(2));
        // ...but when the unplated ones are as big as anything there is, they are used even if too small
        let all_small = [av(32, true), av(48, true), av(44, false)];
        assert_eq!(choose_asset(&all_small, 103), Some(1));
        assert_eq!(choose_asset(&[], 44), None);
        // equal sizes: the first listed
        assert_eq!(choose_asset(&[av(48, false), av(48, false)], 44), Some(0));
    }

    #[test]
    fn the_form_of_the_logo_does_not_change_with_the_size_needed() {
        // unplated files up to 48 px and plated ones up to 176: the dock's size, the monitor's scale or the magnification must not
        // swap the glyph for the plate (or back) for the same app
        let upto48 = [av(16, true), av(24, true), av(48, true), av(44, false), av(88, false), av(176, false)];
        // a typical package made for the taskbar
        let typical = [av(44, false), av(88, false), av(176, false), av(24, true), av(48, true), av(256, true)];
        // nothing unplated, and unplated files that are all there is
        let plain = [av(44, false), av(88, false)];
        let only = [av(32, true), av(48, true)];
        for (name, v) in [("upto48", &upto48[..]), ("typical", &typical[..]), ("plain", &plain[..]), ("only", &only[..])] {
            let forms: std::collections::HashSet<bool> = (16..=512).map(|need| v[choose_asset(v, need).unwrap()].unplated).collect();
            assert_eq!(forms.len(), 1, "{name}: one form for every size needed, got {forms:?}");
        }
        assert!(!upto48[choose_asset(&upto48, 40).unwrap()].unplated, "plated files that are bigger win over unplated ones that stop at 48");
        assert!(typical[choose_asset(&typical, 40).unwrap()].unplated);
    }

    const MANIFEST: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10" xmlns:uap="http://schemas.microsoft.com/appx/manifest/uap/windows10">
  <Identity Name="Vendor.Thing" Publisher="CN=Vendor" Version="1.2.3.4" />
  <Properties><Logo>Assets\StoreLogo.png</Logo></Properties>
  <Applications>
    <Application Id="Other" Executable="other.exe" EntryPoint="Windows.FullTrustApplication">
      <uap:VisualElements DisplayName="Other" Square150x150Logo="Assets\Other150.png" Square44x44Logo="Assets\Other44.png" BackgroundColor="transparent" />
    </Application>
    <Application Id="App" Executable="thing.exe" EntryPoint="Windows.FullTrustApplication">
      <uap:VisualElements DisplayName="Thing" Square150x150Logo="Assets\Square150x150Logo.png" Square44x44Logo="Assets\Square44x44Logo.png" BackgroundColor="transparent">
        <uap:DefaultTile Wide310x150Logo="Assets\Wide.png" />
      </uap:VisualElements>
    </Application>
  </Applications>
</Package>"#;

    #[test]
    fn the_manifest_names_the_logo_of_the_application_asked_for() {
        let m = |id: &str| manifest_logo(MANIFEST, id);
        let logo = |p: &str, b| Some(ManifestLogo { relative: p.into(), base_px: b });
        assert_eq!(m("App"), logo(r"Assets\Square44x44Logo.png", 44));
        assert_eq!(m("app"), logo(r"Assets\Square44x44Logo.png", 44), "an id the dock keeps in lower case");
        assert_eq!(m("Other"), logo(r"Assets\Other44.png", 44));
        assert_eq!(m("Missing"), logo(r"Assets\Other44.png", 44), "an id the manifest does not list: the first application");
        // an older package has only the 30 px and the tile's logo
        let old = r#"<Package><Applications><Application Id="X"><VisualElements Square150x150Logo="a\t150.png" Square30x30Logo="a\s30.png"/></Application></Applications></Package>"#;
        assert_eq!(manifest_logo(old, "x"), logo(r"a\s30.png", 30));
        let tile = r#"<Package><Applications><Application Id="X"><VisualElements Square150x150Logo="a\t150.png"/></Application></Applications></Package>"#;
        assert_eq!(manifest_logo(tile, "x"), logo(r"a\t150.png", 150));
        // nothing usable: no answer, no panic
        assert_eq!(manifest_logo("<Package/>", "x"), None);
        assert_eq!(manifest_logo("not xml at all <", "x"), None);
        assert_eq!(manifest_logo(r#"<Package><Applications><Application Id="X"><VisualElements Square44x44Logo=" "/></Application></Applications></Package>"#, "x"), None);
    }

    #[test]
    fn a_logo_path_splits_into_folder_name_and_extension() {
        assert_eq!(split_logo_path(r"Assets\Square44x44Logo.png"), Some(("Assets".into(), "Square44x44Logo".into(), "png".into())));
        assert_eq!(split_logo_path("Assets/Sub/Logo.png"), Some(("Assets/Sub".into(), "Logo".into(), "png".into())));
        assert_eq!(split_logo_path("Logo.png"), Some((String::new(), "Logo".into(), "png".into())));
        assert_eq!(split_logo_path("Logo"), None);
        assert_eq!(split_logo_path(r"Assets\.png"), None);
    }

    #[test]
    fn the_newest_installed_version_of_a_package_is_taken() {
        let n = |s: &[&str]| newest_package(&s.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(n(&["A_1.0.0.0_x64__p", "A_1.10.0.0_x64__p", "A_1.9.0.0_x64__p"]), Some(1), "10 is more than 9");
        assert_eq!(n(&["A_2.0.0.0_x64__p"]), Some(0));
        assert_eq!(n(&["broken", "A_1.0.0.0_x64__p"]), Some(1));
        assert_eq!(n(&[]), None);
    }

    fn fr(w: u32, bpp: u16) -> FrameInfo {
        FrameInfo { width: w, height: w, bpp }
    }

    #[test]
    fn the_frame_is_the_smallest_that_is_big_enough_else_the_biggest() {
        // the directory writes 256 as 0
        let frames = [fr(16, 32), fr(32, 32), fr(48, 32), fr(0, 32)];
        assert_eq!(frames[3].side(), 256);
        assert_eq!(choose_frame(&frames, 69), Some(3));
        assert_eq!(choose_frame(&frames, 44), Some(2));
        assert_eq!(choose_frame(&frames, 48), Some(2));
        assert_eq!(choose_frame(&frames, 49), Some(3));
        assert_eq!(choose_frame(&frames, 16), Some(0));
        // an old program with nothing above 32: the biggest, to be enlarged by us
        let old = [fr(16, 32), fr(32, 32)];
        assert_eq!(choose_frame(&old, 103), Some(1));
        // the deeper colour of two frames of one size
        let depth = [fr(32, 4), fr(32, 32), fr(32, 8)];
        assert_eq!(choose_frame(&depth, 32), Some(1));
        assert_eq!(choose_frame(&depth, 100), Some(1));
        assert_eq!(choose_frame(&[], 44), None);
    }
}
