//! Id generation and validation for local apps.
//!
//! App ids follow the repo's feature-scoped persisted-id convention (see
//! `tools/cron::generate_cron_task_id`): random lowercase hex minted by the
//! store crate so every entry path (engine command, future FFI) produces the
//! SAME on-disk format. App ids must match `^[a-z0-9][a-z0-9-]{0,63}$` and are
//! validated before ever being used in a path.

use crate::error::AppError;

/// Maximum app id length (regex `{0,63}` tail plus the leading character).
pub const APP_ID_MAX_LEN: usize = 64;

fn random_hex(len: usize) -> String {
    use rand::Rng;
    const ALPHABET: &[u8] = b"0123456789abcdef";
    let mut rng = rand::rng();
    let mut s = String::with_capacity(len);
    for _ in 0..len {
        let idx = rng.random_range(0..ALPHABET.len());
        s.push(ALPHABET[idx] as char);
    }
    s
}

/// Mint a fresh eight-character lowercase-hex app id.
#[must_use]
pub fn generate_app_id() -> String {
    random_hex(8)
}

/// Mint a fresh interaction id (`int-` + 12 lowercase hex chars).
#[must_use]
pub fn generate_interaction_id() -> String {
    format!("int-{}", random_hex(12))
}

/// Mint a fresh suggestion id (`sugg-` + 12 lowercase hex chars).
#[must_use]
pub fn generate_suggestion_id() -> String {
    format!("sugg-{}", random_hex(12))
}

/// True iff `id` matches `^[a-z0-9][a-z0-9-]{0,63}$`.
#[must_use]
pub fn is_valid_app_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    if bytes.is_empty() || bytes.len() > APP_ID_MAX_LEN {
        return false;
    }
    let first_ok = bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit();
    first_ok
        && bytes[1..]
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

/// Validate `id` against the app-id grammar, rejecting anything that could
/// escape the apps root (path separators, `..`, absolute paths are all
/// impossible under the grammar).
pub fn validate_app_id(id: &str) -> Result<(), AppError> {
    if is_valid_app_id(id) {
        Ok(())
    } else {
        Err(AppError::InvalidRequest(format!(
            "invalid app id {id:?}: must match ^[a-z0-9][a-z0-9-]{{0,63}}$"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::AppErrorCode;

    #[test]
    fn generated_app_ids_are_valid_and_hex() {
        for _ in 0..64 {
            let id = generate_app_id();
            assert_eq!(id.len(), 8);
            assert!(id.chars().all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()));
            assert!(is_valid_app_id(&id));
        }
    }

    #[test]
    fn generated_interaction_and_suggestion_ids_are_prefixed() {
        assert!(generate_interaction_id().starts_with("int-"));
        assert!(generate_suggestion_id().starts_with("sugg-"));
        assert_eq!(generate_interaction_id().len(), 16);
        assert_eq!(generate_suggestion_id().len(), 17);
    }

    #[test]
    fn accepts_valid_ids() {
        let max_len = "a".repeat(64);
        for id in ["a", "0", "abc-123", "9-", max_len.as_str()] {
            assert!(is_valid_app_id(id), "expected valid: {id}");
        }
    }

    #[test]
    fn rejects_invalid_and_traversal_ids() {
        let too_long = "a".repeat(65);
        for id in [
            "",
            "-leading-dash",
            "Upper",
            "under_score",
            "spa ce",
            "..",
            "../evil",
            "a/b",
            "a\\b",
            "a.b",
            "über",
            too_long.as_str(),
        ] {
            assert!(!is_valid_app_id(id), "expected invalid: {id}");
            let err = validate_app_id(id).unwrap_err();
            assert_eq!(err.code(), AppErrorCode::InvalidRequest);
        }
    }
}
