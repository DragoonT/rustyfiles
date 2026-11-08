//! rustdecoder: rustyfiles' own video engine. Containers and bitstreams are parsed here; the
//! compressed pictures go to the graphics card's video engine (D3D11 Video). See
//! `docs/design/rustdecoder.md`.

pub mod mp4;

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
}

pub type Result<T> = std::result::Result<T, Error>;
