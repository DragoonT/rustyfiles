//! Our own screen snip (Win+Shift+S is Explorer's, so without Explorer there is none). The Windows half: the geometry and the choices
//! of the overlay as pure code, the capture, the picture formats, the clipboard and the file; the windows themselves are the app's
//! (`rf-app/src/snip/`).
//!
//! | file | what | tested how |
//! |---|---|---|
//! | `geom.rs` | the region a drag makes, across monitors of different scales and negative origins; the monitor and the window under a point | pure |
//! | `model.rs` | the three modes, the drag, what is shown undimmed, what a click or a release takes | pure |
//! | `frame.rs` | the captured monitors in memory and the cut of a region across them | pure |
//! | `encode.rs` | `CF_DIB`, `CF_DIBV5` and PNG bytes from a frame | pure, decoded again |
//! | `save.rs` | `Ekran alıntısı YYYY-AA-GG SSddss.png` in `Pictures\Screenshots`, never overwriting | a folder of the test's own |
//! | `clip.rs` | the order of clipboard calls over a small trait | a fake that records; the real clipboard is never touched by a test |
//! | `capture.rs` | `BitBlt` of every monitor (and the window list), why GDI | the pure window filter; the capture itself by the probe |
//! | `escape.rs` | Escape taken away from the program under the overlay (a hook for that one key, only while the overlay is up) | the pure state machine and the decision; never run against a real keyboard here |
//! | `place.rs` | turns a toolkit window into a topmost cover of one monitor and holds its rectangle | the pure guard; never run against a real window here |
//!
//! WHAT RUNS WHEN. Nothing exists until a snip is asked for: no thread, no hook of its own, no window, no memory. A snip costs one
//! screenshot of every monitor in memory (width x height x 4 bytes each, about 33 MB for a 4K monitor) from the key to the end of the
//! overlay, and nothing after it.

pub mod capture;
pub mod clip;
pub mod encode;
pub mod escape;
pub mod frame;
pub mod geom;
pub mod model;
pub mod place;
pub mod save;
