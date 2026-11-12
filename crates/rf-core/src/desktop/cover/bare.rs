//! Our desktop when there is no Explorer's desktop under it (the "bare" state). Pure.
//!
//! Cover mode was written to sit DIRECTLY ABOVE the shell's desktop window (`zorder`). The user's trial (2026-11-12, Explorer closed for two
//! minutes) showed what it never had to do: run with no shell desktop window at all, and come back to the old rule when Explorer's desktop
//! reappears. What is decided here:
//!
//! * [`state_of`]: bare (no shell desktop window) or beside (there is one);
//! * [`place_for`]: where the windows go in each state - above the desktop, or the bottom of the z-order. (`zorder::target` already
//!   gives the bottom when `Desktop::window` is `None`; [`place_for`] is the same rule stated for the transition code and checked
//!   against it in the tests, so the two cannot drift apart.)
//! * [`fix_after`]: when the shell window came or went, the windows are put in their place AT ONCE, not at the next foreground change.
//! * [`route_for`]: how the wallpaper is read. With Explorer's desktop there, `IDesktopWallpaper` first and the registry second. With
//!   none, the registry alone: the COM object's class has no server path of its own on the machine the cover was written on, so
//!   without Explorer the activation may never return (shell plan test T3, UNVERIFIED) and waiting for it only delays the first picture.
//! * [`backdrop_for`]: what a monitor shows - the picture with its fit mode, or the solid colour - so that the log and the tests say
//!   the same thing the renderer (`rf_win::desktop::render`) does.

use std::path::{Path, PathBuf};

use super::Hwnd;
use super::zorder::{self, Desktop, HWND_BOTTOM};
use crate::desktop::geom::Rgb;
use crate::desktop::placement::Position;
use crate::desktop::wallpaper::{BackgroundKind, Wallpaper};

/// Is there a desktop window of the shell under ours?
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Explorer's (or another shell's) desktop window exists: the windows sit directly above it.
    Beside,
    /// There is none: our windows are the desktop, at the bottom of the z-order.
    Bare,
}

pub fn state_of(shell_window: Option<Hwnd>) -> State {
    if shell_window.is_some() { State::Beside } else { State::Bare }
}

/// Where the windows belong.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// Directly above the shell's desktop window, below every other window.
    AboveDesktop,
    /// The bottom of the z-order, below every program's window; never topmost.
    Bottom,
}

pub fn place_for(state: State) -> Place {
    match state {
        State::Beside => Place::AboveDesktop,
        State::Bare => Place::Bottom,
    }
}

/// What to do about the z-order now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fix {
    Nothing,
    /// Ask every window to be placed again (the window procedure puts each where [`place_for`] says).
    PlaceAgain,
}

/// `before`: the state the windows were last placed for. `now`: the state now. `in_place`: whether the z-order is right for `now`
/// (`zorder::is_in_place`). A change of state is always a fix: the windows that were at the bottom are UNDER the desktop that has just
/// come back (and so invisible), and the ones that were above a desktop that has gone are above programs they should be under.
pub fn fix_after(before: State, now: State, in_place: bool) -> Fix {
    if before != now || !in_place { Fix::PlaceAgain } else { Fix::Nothing }
}

/// How the wallpaper is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// `IDesktopWallpaper` first (bounded), the registry when it does not answer.
    ComThenRegistry,
    /// The registry only.
    RegistryOnly,
}

pub fn route_for(state: State) -> Route {
    match state {
        State::Beside => Route::ComThenRegistry,
        State::Bare => Route::RegistryOnly,
    }
}

/// What one monitor shows under the icons.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Backdrop {
    Picture { path: PathBuf, position: Position },
    Colour(Rgb),
}

/// What monitor number `index` (in the wallpaper's own order) shows. The picture counts only when the background is a picture (not a
/// solid colour), the monitor has one and `readable` says its file can be opened; in every other case the background colour is drawn
/// (never a black or transparent window). A slideshow or Spotlight background shows the picture Windows reports for the monitor.
pub fn backdrop_for(wp: &Wallpaper, index: usize, readable: &dyn Fn(&Path) -> bool) -> Backdrop {
    let picture = wp.pictures().into_iter().nth(index).flatten();
    match picture {
        Some(path) if wp.kind != BackgroundKind::SolidColor && readable(&path) => Backdrop::Picture { path, position: wp.position },
        _ => Backdrop::Colour(wp.background),
    }
}

/// [`place_for`] checked against the window procedure's own rule for a window that is not topmost: the same answer.
pub fn agrees_with_zorder(state: State) -> bool {
    let desktop = match state {
        State::Beside => Desktop { window: Some(7), above: None },
        State::Bare => Desktop { window: None, above: None },
    };
    let t = zorder::target(100, false, &desktop, false);
    match place_for(state) {
        Place::Bottom => t == Some(HWND_BOTTOM),
        // (directly above: behind the window that is above the desktop, or the top of the order when nothing is)
        Place::AboveDesktop => t == Some(zorder::HWND_TOP),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop::geom::Rect;
    use crate::desktop::wallpaper::{MonitorWallpaper, Source};

    fn wp(kind: BackgroundKind, pictures: &[Option<&str>]) -> Wallpaper {
        Wallpaper {
            source: Source::Registry,
            monitors: pictures
                .iter()
                .enumerate()
                .map(|(i, p)| MonitorWallpaper { device: format!("M{i}"), rect: Rect::new(i as i32 * 1920, 0, i as i32 * 1920 + 1920, 1080), picture: p.map(PathBuf::from) })
                .collect(),
            position: Position::Fit,
            position_assumed: false,
            background: Rgb(10, 20, 30),
            kind,
            slideshow: None,
            per_desktop: Vec::new(),
        }
    }

    #[test]
    fn a_shell_window_means_beside_and_none_means_bare() {
        assert_eq!(state_of(Some(0x1234)), State::Beside);
        assert_eq!(state_of(None), State::Bare);
    }

    #[test]
    fn the_windows_go_above_the_desktop_when_there_is_one_and_to_the_bottom_when_there_is_not() {
        assert_eq!(place_for(State::Beside), Place::AboveDesktop);
        assert_eq!(place_for(State::Bare), Place::Bottom);
        // and the window procedure's rule says the same for a window that is not topmost
        assert!(agrees_with_zorder(State::Beside));
        assert!(agrees_with_zorder(State::Bare));
    }

    #[test]
    fn explorers_desktop_coming_or_going_places_the_windows_again_at_once() {
        assert_eq!(fix_after(State::Beside, State::Bare, true), Fix::PlaceAgain, "Explorer left: down to the bottom");
        assert_eq!(fix_after(State::Bare, State::Beside, true), Fix::PlaceAgain, "Explorer is back: up from under its desktop");
        assert_eq!(fix_after(State::Bare, State::Bare, true), Fix::Nothing);
        assert_eq!(fix_after(State::Beside, State::Beside, true), Fix::Nothing);
        assert_eq!(fix_after(State::Bare, State::Bare, false), Fix::PlaceAgain, "the order was disturbed: put back");
    }

    #[test]
    fn a_bare_desktop_reads_the_wallpaper_from_the_registry_alone() {
        assert_eq!(route_for(State::Bare), Route::RegistryOnly);
        assert_eq!(route_for(State::Beside), Route::ComThenRegistry);
    }

    #[test]
    fn a_picture_with_its_fit_mode_is_drawn_when_it_can_be_read() {
        let w = wp(BackgroundKind::Picture, &[Some(r"D:\a.jpg"), Some(r"D:\b.jpg")]);
        assert_eq!(backdrop_for(&w, 0, &|_| true), Backdrop::Picture { path: PathBuf::from(r"D:\a.jpg"), position: Position::Fit });
        assert_eq!(backdrop_for(&w, 1, &|_| true), Backdrop::Picture { path: PathBuf::from(r"D:\b.jpg"), position: Position::Fit });
        let mut filled = w.clone();
        filled.position = Position::Fill;
        assert!(matches!(backdrop_for(&filled, 0, &|_| true), Backdrop::Picture { position: Position::Fill, .. }));
    }

    #[test]
    fn the_colour_is_drawn_for_a_solid_background_no_picture_or_a_picture_that_cannot_be_read() {
        let solid = wp(BackgroundKind::SolidColor, &[Some(r"D:\a.jpg")]);
        assert_eq!(backdrop_for(&solid, 0, &|_| true), Backdrop::Colour(Rgb(10, 20, 30)), "a solid background ignores a picture left in the setting");
        let none = wp(BackgroundKind::Picture, &[None]);
        assert_eq!(backdrop_for(&none, 0, &|_| true), Backdrop::Colour(Rgb(10, 20, 30)));
        let gone = wp(BackgroundKind::Picture, &[Some(r"D:\gone.jpg")]);
        assert_eq!(backdrop_for(&gone, 0, &|_| false), Backdrop::Colour(Rgb(10, 20, 30)), "never a black window");
        // a monitor the wallpaper does not know
        assert_eq!(backdrop_for(&gone, 5, &|_| true), Backdrop::Colour(Rgb(10, 20, 30)));
    }

    #[test]
    fn a_slideshow_or_spotlight_shows_the_picture_windows_reports() {
        for kind in [BackgroundKind::Slideshow, BackgroundKind::Spotlight] {
            let w = wp(kind, &[Some(r"C:\Users\x\Wallpaper.jpg")]);
            assert!(matches!(backdrop_for(&w, 0, &|_| true), Backdrop::Picture { .. }), "{kind:?}");
        }
    }
}
