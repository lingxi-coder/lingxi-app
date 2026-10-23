//! The 6-digit pairing code both sides compute independently and the user
//! compares: the desktop shows it in the approval dialog, the viewer shows it
//! next to "waiting for approval".

use crate::encoding::{b64u_decode, sha256};
use crate::error::ProtocolError;

const DOMAIN: &[u8] = b"lingxi-rc-pair/1";

/// `SHA-256("lingxi-rc-pair/1" || host key || device key || offer nonce || answer nonce)`,
/// first 4 bytes read as a big-endian u32, modulo 1 000 000, zero-padded to six
/// digits. Every input is raw bytes (32, 32, 16 and 16 long).
pub fn pairing_code(
    host_key: &[u8],
    device_key: &[u8],
    offer_nonce: &[u8],
    answer_nonce: &[u8],
) -> Result<String, ProtocolError> {
    if host_key.len() != 32
        || device_key.len() != 32
        || offer_nonce.len() != 16
        || answer_nonce.len() != 16
    {
        return Err(ProtocolError::new(
            "pairing code inputs have the wrong length",
        ));
    }
    let mut input = Vec::with_capacity(DOMAIN.len() + 96);
    input.extend_from_slice(DOMAIN);
    input.extend_from_slice(host_key);
    input.extend_from_slice(device_key);
    input.extend_from_slice(offer_nonce);
    input.extend_from_slice(answer_nonce);
    let digest = sha256(&input);
    let value = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]);
    Ok(format!("{:06}", value % 1_000_000))
}

/// [`pairing_code`] from the four values as the records carry them (`b64u`).
pub fn pairing_code_for(
    device_key: &str,
    offer_nonce: &str,
    host_key: &str,
    answer_nonce: &str,
) -> Result<String, ProtocolError> {
    pairing_code(
        &b64u_decode(host_key)?,
        &b64u_decode(device_key)?,
        &b64u_decode(offer_nonce)?,
        &b64u_decode(answer_nonce)?,
    )
}
