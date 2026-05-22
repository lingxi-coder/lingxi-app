//! 8-character alphanumeric pairing codes (A3).
//!
//! The alphabet deliberately omits `0`, `O`, `1`, and `I` so the codes are
//! easy to read aloud or type on a mobile device. Codes are generated from
//! `rand::rng()` per call — callers are responsible for storing the code and
//! enforcing expiry (see [`crate::pairing::BridgePairing`]).

use rand::Rng;

/// Allowed characters for pairing codes. 32 symbols, no `0`/`O`/`1`/`I`.
const PAIRING_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/// Generate a fresh 8-character pairing code.
///
/// Returns a `String` because the result is destined for human entry. Each
/// character is sampled uniformly from [`PAIRING_ALPHABET`].
#[must_use]
#[allow(clippy::cast_possible_truncation)]
pub fn generate_pairing_code() -> String {
    let mut rng = rand::rng();
    (0..8)
        .map(|_| PAIRING_ALPHABET[rng.random_range(0..PAIRING_ALPHABET.len())] as char)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_code_is_8_chars() {
        let c = generate_pairing_code();
        assert_eq!(c.chars().count(), 8);
    }

    #[test]
    fn pairing_code_has_no_confusable_chars() {
        for _ in 0..100 {
            let c = generate_pairing_code();
            assert!(!c.contains('0'));
            assert!(!c.contains('O'));
            assert!(!c.contains('1'));
            assert!(!c.contains('I'));
        }
    }
}
