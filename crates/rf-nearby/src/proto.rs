//! The wire protocol (also written down in docs/nearby-protocol.md). One QUIC connection per transfer or pairing. The connecting side
//! opens one bidirectional "control" stream and speaks first; messages on it are frames: a 4-byte big-endian length and that many
//! bytes of JSON. File bytes go on unidirectional streams, one per file, each starting with a `FileHeader` frame.

use serde::{Deserialize, Serialize};

pub const PROTOCOL: u32 = 1;
/// The largest frame accepted (an offer of a great many files is the biggest one).
const MAX_FRAME: usize = 64 << 20;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Hello {
    pub proto: u32,
    /// The sender's device id; checked against the certificate it proved.
    pub id: String,
    pub name: String,
    pub os: String,
    pub purpose: Purpose,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Purpose {
    Send { items: Vec<Item> },
    Pair,
}

/// One entry of an offer: a file, or a folder that must exist even when empty. `path` is relative, with `/` between its parts.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Item {
    pub path: String,
    pub size: u64,
    #[serde(default)]
    pub dir: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Answer {
    pub accept: bool,
    #[serde(default)]
    pub reason: Option<String>,
    /// For each item of the offer, how many bytes the receiver already has (a transfer that broke off goes on from there).
    #[serde(default)]
    pub resume: Vec<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileHeader {
    pub index: u32,
    pub offset: u64,
}

/// After the last file's stream: the BLAKE3 of every item (empty for folders), for the receiver to check before it keeps them.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Done {
    pub hashes: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Outcome {
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairConfirm {
    pub ok: bool,
}

pub async fn write_frame<T: Serialize>(send: &mut quinn::SendStream, value: &T) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec(value)?;
    send.write_all(&(bytes.len() as u32).to_be_bytes()).await?;
    send.write_all(&bytes).await?;
    Ok(())
}

pub async fn read_frame<T: for<'de> Deserialize<'de>>(recv: &mut quinn::RecvStream) -> anyhow::Result<T> {
    let mut len = [0u8; 4];
    recv.read_exact(&mut len).await?;
    let len = u32::from_be_bytes(len) as usize;
    anyhow::ensure!(len <= MAX_FRAME, "a frame of {len} bytes is too large");
    let mut buf = vec![0u8; len];
    recv.read_exact(&mut buf).await?;
    Ok(serde_json::from_slice(&buf)?)
}

/// A relative path from a peer, as the parts to join under the destination folder; `None` for anything that could leave it or name
/// something odd on one of the platforms: an absolute path, `..`, `.`, a drive or a stream (`:`), a backslash, an empty part, a
/// control character.
pub fn safe_parts(path: &str) -> Option<Vec<&str>> {
    if path.is_empty() || path.starts_with('/') {
        return None;
    }
    let parts: Vec<&str> = path.split('/').collect();
    let bad = |p: &&str| p.is_empty() || *p == "." || *p == ".." || p.contains(['\\', ':']) || p.chars().any(char::is_control);
    if parts.iter().any(bad) { None } else { Some(parts) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_that_could_leave_the_folder_are_refused() {
        assert_eq!(safe_parts("a/b.txt"), Some(vec!["a", "b.txt"]));
        for bad in ["", "/etc/passwd", "../x", "a/../../x", "a/./b", "C:/x", "a\\b", "a//b", "x:stream", "a\u{0}b"] {
            assert_eq!(safe_parts(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn messages_keep_their_shape_on_the_wire() {
        let h = Hello { proto: 1, id: "x".into(), name: "n".into(), os: "windows".into(), purpose: Purpose::Send { items: vec![Item { path: "a".into(), size: 3, dir: false }] } };
        let json = serde_json::to_string(&h).unwrap();
        assert!(json.contains("\"kind\":\"send\""), "{json}");
        assert_eq!(serde_json::from_str::<Hello>(&json).unwrap(), h);
    }
}
