//! Cryptography of the RTDB tunnel: per-direction AES-256-GCM keys derived from
//! the X25519 shared secret, frame sealing, and the plaintext frame layout.
//!
//! A frame is stored in RTDB as `b64u(ciphertext || tag)` under key `f{seq}`.
//! The nonce is four zero bytes followed by `seq` as a big-endian u64, so a
//! (key, nonce) pair never repeats as long as `seq` does not; the AAD binds the
//! connection, the direction and the sequence number, so a frame cannot be
//! replayed on another connection, in the other direction or at another position.

use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM};

use crate::encoding::{b64u_decode, b64u_encode};
use crate::error::ProtocolError;
use crate::keys::hkdf_sha256;
use crate::version::{FRAGMENT_MAX_BYTES, TUNNEL_FRAME_PLAINTEXT_MAX};

const LABEL: &str = "lingxi-rc-tunnel/1";
const TAG_BYTES: usize = 16;
/// 49152 sealed bytes encode to 65536 characters; the RTDB rules enforce the same bound.
const STORED_MAX_CHARS: usize = 65536;

/// `Up`: viewer to host. `Down`: host to viewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TunnelDir {
    /// Viewer to host.
    Up,
    /// Host to viewer.
    Down,
}

impl TunnelDir {
    /// The wire spelling, used in the HKDF info and the AAD.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Down => "down",
        }
    }
}

/// An AES-256-GCM traffic key of one direction. Never printed.
pub struct TunnelKey(LessSafeKey);

impl std::fmt::Debug for TunnelKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TunnelKey(..)")
    }
}

impl TunnelKey {
    /// Wrap 32 raw key bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let unbound = UnboundKey::new(&AES_256_GCM, bytes)
            .map_err(|_| ProtocolError::new("tunnel key must be 32 bytes"))?;
        Ok(Self(LessSafeKey::new(unbound)))
    }
}

/// The two traffic keys of one connection.
#[derive(Debug)]
pub struct TunnelKeys {
    /// Seals viewer-to-host frames.
    pub up: TunnelKey,
    /// Seals host-to-viewer frames.
    pub down: TunnelKey,
}

/// A tunnel frame failed authentication or was not a well-formed sealed value.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct TunnelCryptoError(pub String);

/// `HKDF-SHA256(ikm = shared, salt = UTF-8(conn_id), info = "lingxi-rc-tunnel/1 up" | "... down")`,
/// 32 bytes each, as AES-GCM keys.
pub fn derive_tunnel_keys(shared: &[u8], conn_id: &str) -> Result<TunnelKeys, ProtocolError> {
    let derive = |dir: TunnelDir| {
        let info = format!("{LABEL} {}", dir.as_str());
        let material = hkdf_sha256(shared, conn_id.as_bytes(), info.as_bytes(), 32);
        TunnelKey::from_bytes(&material)
    };
    Ok(TunnelKeys {
        up: derive(TunnelDir::Up)?,
        down: derive(TunnelDir::Down)?,
    })
}

/// The 12-byte GCM nonce of frame `seq`: four zero bytes, then `seq` as a big-endian u64.
pub fn tunnel_nonce(seq: u64) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[4..].copy_from_slice(&seq.to_be_bytes());
    nonce
}

/// The additional authenticated data of frame `seq`: `"lingxi-rc-tunnel/1\n{conn_id}\n{dir}\n{seq}"`.
pub fn tunnel_aad(conn_id: &str, dir: TunnelDir, seq: u64) -> Vec<u8> {
    format!("{LABEL}\n{conn_id}\n{}\n{seq}", dir.as_str()).into_bytes()
}

/// Encrypt one frame's plaintext (at most [`TUNNEL_FRAME_PLAINTEXT_MAX`] bytes) into its stored value.
pub fn seal_frame(
    key: &TunnelKey,
    conn_id: &str,
    dir: TunnelDir,
    seq: u64,
    plaintext: &[u8],
) -> Result<String, ProtocolError> {
    if plaintext.len() > TUNNEL_FRAME_PLAINTEXT_MAX {
        return Err(ProtocolError::new("tunnel frame plaintext is too large"));
    }
    let aad = tunnel_aad(conn_id, dir, seq);
    let mut buffer = plaintext.to_vec();
    key.0
        .seal_in_place_append_tag(
            Nonce::assume_unique_for_key(tunnel_nonce(seq)),
            Aad::from(aad),
            &mut buffer,
        )
        .map_err(|_| ProtocolError::new("tunnel frame could not be sealed"))?;
    Ok(b64u_encode(&buffer))
}

/// Decrypt a stored frame value. Fails with [`TunnelCryptoError`] when the value
/// is malformed, or when it does not authenticate under this key, connection,
/// direction and sequence number.
pub fn open_frame(
    key: &TunnelKey,
    conn_id: &str,
    dir: TunnelDir,
    seq: u64,
    stored: &str,
) -> Result<Vec<u8>, TunnelCryptoError> {
    if stored.len() > STORED_MAX_CHARS {
        return Err(TunnelCryptoError("tunnel frame is too long".into()));
    }
    let mut sealed = b64u_decode(stored)
        .map_err(|_| TunnelCryptoError("tunnel frame is not base64url".into()))?;
    if sealed.len() < TAG_BYTES || sealed.len() > TUNNEL_FRAME_PLAINTEXT_MAX + TAG_BYTES {
        return Err(TunnelCryptoError(
            "tunnel frame has an impossible size".into(),
        ));
    }
    let aad = tunnel_aad(conn_id, dir, seq);
    let plaintext_len = key
        .0
        .open_in_place(
            Nonce::assume_unique_for_key(tunnel_nonce(seq)),
            Aad::from(aad),
            &mut sealed,
        )
        .map_err(|_| TunnelCryptoError("tunnel frame failed authentication".into()))?
        .len();
    sealed.truncate(plaintext_len);
    Ok(sealed)
}

/// Lay fragments out as a frame plaintext: repeated `[u32 BE length][fragment]`.
/// The total may not exceed [`TUNNEL_FRAME_PLAINTEXT_MAX`].
pub fn pack_fragments(fragments: &[Vec<u8>]) -> Result<Vec<u8>, ProtocolError> {
    let total: usize = fragments.iter().map(|f| 4 + f.len()).sum();
    if total > TUNNEL_FRAME_PLAINTEXT_MAX {
        return Err(ProtocolError::new("tunnel frame plaintext is too large"));
    }
    let mut out = Vec::with_capacity(total);
    for fragment in fragments {
        let length =
            u32::try_from(fragment.len()).map_err(|_| ProtocolError::new("fragment too large"))?;
        out.extend_from_slice(&length.to_be_bytes());
        out.extend_from_slice(fragment);
    }
    Ok(out)
}

/// Inverse of [`pack_fragments`]. Rejects an oversized plaintext, a truncated
/// length or body, and fragments that are empty or longer than [`FRAGMENT_MAX_BYTES`].
pub fn unpack_fragments(plaintext: &[u8]) -> Result<Vec<Vec<u8>>, ProtocolError> {
    if plaintext.len() > TUNNEL_FRAME_PLAINTEXT_MAX {
        return Err(ProtocolError::new("tunnel frame plaintext is too large"));
    }
    let mut fragments = Vec::new();
    let mut rest = plaintext;
    while !rest.is_empty() {
        let Some((prefix, tail)) = rest.split_first_chunk::<4>() else {
            return Err(ProtocolError::new("tunnel frame is truncated"));
        };
        let length = u32::from_be_bytes(*prefix) as usize;
        if length == 0 || length > FRAGMENT_MAX_BYTES {
            return Err(ProtocolError::new(
                "tunnel frame holds a fragment of impossible size",
            ));
        }
        if tail.len() < length {
            return Err(ProtocolError::new("tunnel frame is truncated"));
        }
        let (fragment, after) = tail.split_at(length);
        fragments.push(fragment.to_vec());
        rest = after;
    }
    Ok(fragments)
}
