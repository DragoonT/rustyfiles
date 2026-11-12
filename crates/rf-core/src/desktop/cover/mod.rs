//! The decisions of "cover mode" (shell plan increment F4): our desktop drawn in one borderless window per monitor, laid over
//! Explorer's desktop while Explorer is still the shell. Nothing here touches the system; the thin Win32 layer that carries the
//! decisions out is `rf_win::desktop::cover` and `rf_win::desktop::showdesktop`.
//!
//! | file | what it decides |
//! |---|---|
//! | `geometry` | the window's rectangle for a monitor: the monitor less ONE pixel row, and why |
//! | `zorder` | where the window may sit: directly above the shell's desktop window, below every other window, whatever is asked of it |
//! | `activation` | which presses may activate the window (an icon: yes; empty space: no) |
//! | `plan` | monitors added, removed or changed -> windows to create, remove, place again; the debounce that waits for a hot-plug to settle |
//! | `bare` | the state with NO shell desktop window (Explorer away): the windows at the bottom, the wallpaper read from the registry, what is drawn |
//! | `showdesktop` | Show Desktop and Minimize All: which windows to minimize, what to restore and in which order |

pub mod activation;
pub mod bare;
pub mod geometry;
pub mod plan;
pub mod showdesktop;
pub mod zorder;

/// A window handle as the rest of the code carries it (`HWND.0 as isize`).
pub type Hwnd = isize;
