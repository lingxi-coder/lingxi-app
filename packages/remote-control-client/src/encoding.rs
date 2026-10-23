//! Byte and text helpers: strict unpadded base64url, hashing, randomness.

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine as _;
use ring::digest;
use ring::rand::{SecureRandom, SystemRandom};

use crate::error::ProtocolError;

/// Encode bytes as unpadded base64url (the protocol's `b64u`).
pub fn b64u_encode(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Decode unpadded base64url. Strict: rejects characters outside the alphabet
/// (including `=` padding, `+`, `/` and whitespace), impossible lengths and
/// non-zero trailing bits.
pub fn b64u_decode(text: &str) -> Result<Vec<u8>, ProtocolError> {
    URL_SAFE_NO_PAD
        .decode(text.as_bytes())
        .map_err(|e| ProtocolError::new(format!("base64url: {e}")))
}

/// Encode bytes as standard padded base64 (the `ImageRefDto.base64` form).
pub fn b64_encode(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

/// Decode standard padded base64.
pub fn b64_decode(text: &str) -> Result<Vec<u8>, ProtocolError> {
    STANDARD
        .decode(text.as_bytes())
        .map_err(|e| ProtocolError::new(format!("base64: {e}")))
}

/// `true` when `text` is valid unpadded base64url of exactly `byte_length` bytes.
pub fn is_b64u(text: &str, byte_length: usize) -> bool {
    text.len() == b64u_length(byte_length) && b64u_decode(text).is_ok()
}

/// Length of the unpadded base64url encoding of `byte_length` bytes.
pub fn b64u_length(byte_length: usize) -> usize {
    byte_length / 3 * 4
        + match byte_length % 3 {
            0 => 0,
            n => n + 1,
        }
}

/// SHA-256 digest.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let digest = digest::digest(&digest::SHA256, data);
    let mut out = [0u8; 32];
    out.copy_from_slice(digest.as_ref());
    out
}

/// Fill `out` with cryptographically secure random bytes.
///
/// # Panics
/// Panics if the operating system's random source fails, which leaves no safe way to continue.
pub fn fill_random(out: &mut [u8]) {
    SystemRandom::new()
        .fill(out)
        .expect("the operating system random source failed");
}

/// `N` random bytes.
pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    fill_random(&mut out);
    out
}

/// A fresh `conn_id`-style identifier: 16 random bytes as 22 base64url characters.
pub fn random_id() -> String {
    b64u_encode(&random_bytes::<16>())
}

/// A random number in `[0, 1)`, for jitter.
pub fn random_unit() -> f64 {
    let bits = u64::from_be_bytes(random_bytes::<8>()) >> 11;
    // 53 random bits: exactly representable, uniformly in [0, 1).
    bits as f64 / (1u64 << 53) as f64
}

/// Content equality of two byte strings (constant time for equal lengths).
pub fn bytes_equal(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
