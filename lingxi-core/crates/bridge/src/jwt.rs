//! Project-scoped JWT (HS256) for the IDE bridge (A3).
//!
//! A paired IDE device presents a JWT on connect; the engine verifies the
//! signature, expiry, and project binding before accepting the connection.
//! Keeping the JWT scoped to a single `project_dir` means a stolen token
//! cannot be replayed against a different workspace on the same host.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use thiserror::Error;

type HmacSha256 = Hmac<Sha256>;

/// Claims carried inside a bridge JWT.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JwtClaims {
    /// Subject — the paired device identifier.
    pub sub: String,
    /// Project directory this token is scoped to.
    pub project_dir: String,
    /// Expiry as Unix seconds since the epoch.
    pub exp: u64,
    /// Issued-at time as Unix seconds since the epoch.
    pub iat: u64,
}

/// Errors raised by [`JwtVerifier::verify`].
#[derive(Debug, Clone, Error)]
pub enum JwtError {
    /// The token did not parse as `header.payload.signature`.
    #[error("invalid format")]
    InvalidFormat,
    /// HMAC signature did not match — token was tampered with or the secret
    /// is wrong.
    #[error("invalid signature")]
    InvalidSignature,
    /// The token's `exp` is in the past.
    #[error("expired")]
    Expired,
    /// The token's `project_dir` does not match the requested one.
    #[error("project_dir mismatch")]
    ProjectMismatch,
}

/// Sign and verify project-scoped bridge JWTs using HS256.
///
/// Stateless beyond the embedded shared secret.
pub struct JwtVerifier {
    secret: Vec<u8>,
}

impl JwtVerifier {
    /// Construct a verifier bound to the given shared secret bytes.
    #[must_use]
    pub fn new(secret: Vec<u8>) -> Self {
        Self { secret }
    }

    /// Sign `claims` and return a compact-encoded JWT.
    #[must_use]
    pub fn sign(&self, claims: &JwtClaims) -> String {
        let header = URL_SAFE_NO_PAD.encode(b"{\"alg\":\"HS256\",\"typ\":\"JWT\"}");
        let payload =
            URL_SAFE_NO_PAD.encode(serde_json::to_string(claims).expect("serialize claims"));
        let to_sign = format!("{header}.{payload}");
        let mut mac = HmacSha256::new_from_slice(&self.secret).expect("hmac key");
        mac.update(to_sign.as_bytes());
        let sig = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
        format!("{to_sign}.{sig}")
    }

    /// Verify `token` against `project_dir` and the current epoch second
    /// `now_secs`. Returns the decoded claims on success.
    pub fn verify(
        &self,
        token: &str,
        project_dir: &str,
        now_secs: u64,
    ) -> Result<JwtClaims, JwtError> {
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() != 3 {
            return Err(JwtError::InvalidFormat);
        }
        let to_verify = format!("{}.{}", parts[0], parts[1]);
        let mut mac =
            HmacSha256::new_from_slice(&self.secret).map_err(|_| JwtError::InvalidSignature)?;
        mac.update(to_verify.as_bytes());
        let expected_sig = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
        if expected_sig != parts[2] {
            return Err(JwtError::InvalidSignature);
        }

        let payload_bytes = URL_SAFE_NO_PAD
            .decode(parts[1])
            .map_err(|_| JwtError::InvalidFormat)?;
        let claims: JwtClaims =
            serde_json::from_slice(&payload_bytes).map_err(|_| JwtError::InvalidFormat)?;
        if claims.exp < now_secs {
            return Err(JwtError::Expired);
        }
        if claims.project_dir != project_dir {
            return Err(JwtError::ProjectMismatch);
        }
        Ok(claims)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_then_verify_roundtrip() {
        let v = JwtVerifier::new(b"my-secret".to_vec());
        let claims = JwtClaims {
            sub: "dev1".into(),
            project_dir: "/proj".into(),
            exp: 99_999_999_999,
            iat: 1,
        };
        let tok = v.sign(&claims);
        let out = v.verify(&tok, "/proj", 100).unwrap();
        assert_eq!(out.sub, "dev1");
    }

    #[test]
    fn tampered_token_rejected() {
        let v = JwtVerifier::new(b"my-secret".to_vec());
        let claims = JwtClaims {
            sub: "dev1".into(),
            project_dir: "/proj".into(),
            exp: 99_999_999_999,
            iat: 1,
        };
        let mut tok = v.sign(&claims);
        tok.push('A');
        assert!(matches!(
            v.verify(&tok, "/proj", 100),
            Err(JwtError::InvalidSignature)
        ));
    }

    #[test]
    fn wrong_project_dir_rejected() {
        let v = JwtVerifier::new(b"s".to_vec());
        let claims = JwtClaims {
            sub: "dev1".into(),
            project_dir: "/a".into(),
            exp: 99_999_999_999,
            iat: 1,
        };
        let tok = v.sign(&claims);
        assert!(matches!(
            v.verify(&tok, "/b", 100),
            Err(JwtError::ProjectMismatch)
        ));
    }
}
