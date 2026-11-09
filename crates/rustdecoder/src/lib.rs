//! rustdecoder: rustyfiles' own video engine. Containers and bitstreams are parsed here; the
//! compressed pictures go to the graphics card's video engine (D3D11 Video). See
//! `docs/design/rustdecoder.md`.

pub mod bits;
pub mod dpb;
pub mod hevc;
pub mod mp4;
pub mod nal;
pub mod slice;

#[cfg(windows)]
pub mod d3d11;
#[cfg(windows)]
pub mod decoder;
pub mod dxva;
#[cfg(windows)]
pub mod player;
#[cfg(windows)]
pub mod present;

#[cfg(windows)]
pub use decoder::{Frame, HevcDecoder, StreamInfo};

/// Errors from reading a file.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// The file breaks the format (a box runs past its parent, a table is cut short, …).
    #[error("invalid file: {0}")]
    Invalid(&'static str),
    /// Valid, but something this reader doesn't handle yet.
    #[error("not supported yet: {0}")]
    Unsupported(&'static str),
    /// The graphics driver refused something.
    #[error("GPU: {0}")]
    Gpu(String),
}

pub type Result<T> = std::result::Result<T, Error>;
