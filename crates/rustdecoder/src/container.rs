//! Opens a media file's index whatever the container: MP4 / MOV or Matroska / WebM.

use std::io::{Read, Seek, SeekFrom};

use crate::mp4::Movie;
use crate::{Result, avi, mkv, mp4, ts};

/// Reads the track tables of `r`, picking the reader from the first bytes.
pub fn read<R: Read + Seek>(r: &mut R) -> Result<Movie> {
    let mut head = [0u8; 1024];
    r.seek(SeekFrom::Start(0))?;
    let n = r.read(&mut head)?;
    r.seek(SeekFrom::Start(0))?;
    if mkv::is_ebml(&head[..n]) {
        mkv::read(r)
    } else if ts::detect(&head[..n]).is_some() {
        ts::read(r)
    } else if avi::is_avi(&head[..n]) {
        avi::read(r)
    } else {
        mp4::read(r)
    }
}
