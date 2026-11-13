//! The model of the desktop that rustyfiles draws itself (shell plan, increments F1 and F2): what is on the wallpaper, what is on the
//! desktop, and where each icon sits. Nothing in this module touches the system: the registry is read through [`registry::RegRead`], the
//! folders through traits in rf-win, so every rule here is tested on plain data. The system side is `rf_win::desktop`.
//!
//! | file | what it is |
//! |---|---|
//! | `geom` | rectangles, sizes, colours |
//! | `placement` | the wallpaper maths: for a picture, a monitor and a position mode, which part of the picture goes where, the colour bars, the tile origin |
//! | `registry` | the read-only registry trait and an in-memory fake |
//! | `wallpaper` | the wallpaper settings as plain data; the registry fallback; the per-virtual-desktop value; change detection |
//! | `virtuals` | This PC, Recycle Bin, Network, user's files, Control Panel: which ones the user has enabled, and the app's own choice on top of Windows' |
//! | `arrival` | where a new, pasted or dropped item goes: next to the pointer (not with auto arrange) |
//! | `items` | the merged list of the user's Desktop and the Public one, duplicates, hidden / system rules, the diff that tells added / removed / renamed |
//! | `grid` | icon size steps and the grid metrics at every scale; cells; column-major order |
//! | `layout` | the rules that give every item a cell on a monitor layout (first free cell, collisions, resolution change, auto arrange) |
//! | `store` | the layout file (`desktop-layout.json`): format, atomic write, corruption, newer versions |
//! | `import` | turning Explorer's icon positions (as read from its view) into our cells, refusing when the numbers do not fit the monitors |

pub mod arrival;
pub mod corners;
pub mod cover;
pub mod geom;
pub mod grid;
pub mod import;
pub mod items;
pub mod layout;
pub mod placement;
pub mod registry;
pub mod store;
pub mod virtuals;
pub mod wallpaper;

pub use geom::{Rect, Rgb, Size};
