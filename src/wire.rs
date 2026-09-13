//! Length-prefixed framing over an async byte stream.
//!
//! Every message exchanged between hops is a frame: a 4-byte big-endian
//! length followed by that many bytes of payload. The payload is either a
//! raw handshake blob (during circuit setup) or an AEAD ciphertext (a
//! sealed [`crate::cell::Cell`]).

use anyhow::{bail, Result};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Hard cap on a single frame so a peer cannot make us allocate unbounded
/// memory. Cells carry at most ~one read buffer of app data plus overhead.
pub const MAX_FRAME_LEN: usize = 1 << 20; // 1 MiB

/// Read one length-prefixed frame, returning its payload bytes (prefix stripped).
pub async fn read_frame<R>(r: &mut R) -> Result<Vec<u8>>
where
    R: AsyncRead + Unpin,
{
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len == 0 {
        bail!("received zero-length frame");
    }
    if len > MAX_FRAME_LEN {
        bail!("frame length {len} exceeds cap {MAX_FRAME_LEN}");
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    return Ok(buf);
}

/// Write `payload` as a length-prefixed frame and flush.
pub async fn write_frame<W>(w: &mut W, payload: &[u8]) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    if payload.is_empty() {
        bail!("refusing to write empty frame");
    }
    if payload.len() > MAX_FRAME_LEN {
        bail!("frame length {} exceeds cap {MAX_FRAME_LEN}", payload.len());
    }
    let len = (payload.len() as u32).to_be_bytes();
    w.write_all(&len).await?;
    w.write_all(payload).await?;
    w.flush().await?;
    return Ok(());
}

/// Prepend a 4-byte big-endian length to `payload`, producing the exact bytes
/// that would go on the wire. Used when a relay must carry an already-framed
/// blob inside a `Relay` cell's payload.
pub fn frame_bytes(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + payload.len());
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    return out;
}

/// Inverse of [`frame_bytes`]: strip the 4-byte length prefix, returning the
/// inner payload. Validates the prefix matches the remaining length.
pub fn strip_len(framed: &[u8]) -> Result<Vec<u8>> {
    if framed.len() < 4 {
        bail!("framed blob too short ({} bytes)", framed.len());
    }
    let len = u32::from_be_bytes([framed[0], framed[1], framed[2], framed[3]]) as usize;
    if len != framed.len() - 4 {
        bail!("length prefix {len} != actual {}", framed.len() - 4);
    }
    return Ok(framed[4..].to_vec());
}
