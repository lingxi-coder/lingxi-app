//! Lightweight AWS credential discovery for `SigV4Authenticator`.
//!
//! Resolves static credentials from a layered provider chain WITHOUT the heavy
//! `aws-config` SDK runtime (which cannot build on the pinned Rust 1.82 toolchain
//! — it requires `aws-smithy-types ^1.4.8` / rustc 1.91 and pulls the
//! `edition2024`-gated `const-oid`). The chain, in order:
//!
//! 1. **Environment** — `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY`
//!    (+ optional `AWS_SESSION_TOKEN`).
//! 2. **Shared credentials file** — `$AWS_SHARED_CREDENTIALS_FILE` or
//!    `~/.aws/credentials`, the `[$AWS_PROFILE]` (or `[default]`) section.
//! 3. **`credential_process`** — `$AWS_CONFIG_FILE` or `~/.aws/config`, the
//!    profile's `credential_process` command (this is how `aws configure sso`
//!    / `aws-vault` typically surface short-lived SSO credentials).
//! 4. **IMDSv2** — the EC2 instance metadata service (token + role creds),
//!    issued over the injected [`HttpTransport`]. Skipped when no transport.
//!
//! Native `sso_session` config (the cached OIDC token in `~/.aws/sso/cache`) is
//! intentionally out of scope — use `credential_process` or run `aws sso login`.

use api_client::ApiError;
use protocol::{HttpMethod, HttpRequest};
use std::sync::Arc;
use std::time::Duration;
use traits::HttpTransport;

/// Resolved static AWS credentials for SigV4 signing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AwsCreds {
    /// `AWS_ACCESS_KEY_ID`.
    pub access_key: String,
    /// `AWS_SECRET_ACCESS_KEY`.
    pub secret_key: String,
    /// Optional `AWS_SESSION_TOKEN` (set for STS / SSO / IMDS credentials).
    pub session_token: Option<String>,
}

/// Read credentials from the environment, if both required vars are present.
#[must_use]
pub fn from_env() -> Option<AwsCreds> {
    let access_key = std::env::var("AWS_ACCESS_KEY_ID").ok()?;
    let secret_key = std::env::var("AWS_SECRET_ACCESS_KEY").ok()?;
    if access_key.is_empty() || secret_key.is_empty() {
        return None;
    }
    Some(AwsCreds {
        access_key,
        secret_key,
        session_token: std::env::var("AWS_SESSION_TOKEN").ok().filter(|s| !s.is_empty()),
    })
}

/// Parse the `[profile]` section of an AWS shared-credentials INI file.
///
/// Section headers are bare profile names (`[default]`, `[work]`). Keys
/// recognised: `aws_access_key_id`, `aws_secret_access_key`, `aws_session_token`.
/// Returns `None` unless both the access key and secret are present.
#[must_use]
pub fn parse_ini_credentials(content: &str, profile: &str) -> Option<AwsCreds> {
    let mut in_section = false;
    let (mut access, mut secret, mut token) = (None, None, None);
    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            // Config-file style `[profile x]` collapses to `x`; credentials
            // file uses the bare name.
            let name = name.strip_prefix("profile ").unwrap_or(name).trim();
            in_section = name == profile;
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            match k.trim() {
                "aws_access_key_id" => access = Some(v.trim().to_string()),
                "aws_secret_access_key" => secret = Some(v.trim().to_string()),
                "aws_session_token" => token = Some(v.trim().to_string()),
                _ => {}
            }
        }
    }
    Some(AwsCreds {
        access_key: access?,
        secret_key: secret?,
        session_token: token,
    })
}

/// Find a profile's `credential_process` command in an AWS config INI file.
#[must_use]
pub fn parse_credential_process(content: &str, profile: &str) -> Option<String> {
    let mut in_section = false;
    for raw in content.lines() {
        let line = raw.trim();
        if let Some(name) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            let name = name.strip_prefix("profile ").unwrap_or(name).trim();
            in_section = name == profile;
            continue;
        }
        if in_section {
            if let Some((k, v)) = line.split_once('=') {
                if k.trim() == "credential_process" {
                    return Some(v.trim().to_string());
                }
            }
        }
    }
    None
}

/// Parse the JSON emitted by a `credential_process` command (the
/// `AccessKeyId`/`SecretAccessKey`/`SessionToken` schema, version 1) or by the
/// IMDS `security-credentials` endpoint (`Token` instead of `SessionToken`).
#[must_use]
pub fn parse_json_credentials(json: &str) -> Option<AwsCreds> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let access_key = v.get("AccessKeyId")?.as_str()?.to_string();
    let secret_key = v.get("SecretAccessKey")?.as_str()?.to_string();
    let session_token = v
        .get("SessionToken")
        .or_else(|| v.get("Token"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    Some(AwsCreds {
        access_key,
        secret_key,
        session_token,
    })
}

fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
}

fn active_profile() -> String {
    std::env::var("AWS_PROFILE").unwrap_or_else(|_| "default".to_string())
}

fn shared_credentials_path() -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("AWS_SHARED_CREDENTIALS_FILE") {
        return Some(std::path::PathBuf::from(p));
    }
    home_dir().map(|h| h.join(".aws").join("credentials"))
}

fn config_path() -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("AWS_CONFIG_FILE") {
        return Some(std::path::PathBuf::from(p));
    }
    home_dir().map(|h| h.join(".aws").join("config"))
}

/// Fetch instance-role credentials over IMDSv2 using the injected transport.
///
/// Uses a short timeout per call so a non-EC2 host fails fast. Returns `None`
/// on any error (treated as "not on EC2 / no role").
async fn from_imds(transport: &Arc<dyn HttpTransport>) -> Option<AwsCreds> {
    let short = Some(Duration::from_secs(1));
    // 1. Get a session token (IMDSv2).
    let token_req = HttpRequest {
        method: HttpMethod::Put,
        url: "http://169.254.169.254/latest/api/token".to_string(),
        headers: vec![(
            "x-aws-ec2-metadata-token-ttl-seconds".to_string(),
            "21600".to_string(),
        )],
        body: None,
        timeout: short,
    };
    let token = transport.request(token_req).await.ok()?;
    if token.status != 200 || token.body.is_empty() {
        return None;
    }
    let hdr = vec![("x-aws-ec2-metadata-token".to_string(), token.body.clone())];
    // 2. Discover the attached role name.
    let role_req = HttpRequest {
        method: HttpMethod::Get,
        url: "http://169.254.169.254/latest/meta-data/iam/security-credentials/".to_string(),
        headers: hdr.clone(),
        body: None,
        timeout: short,
    };
    let role = transport.request(role_req).await.ok()?;
    if role.status != 200 {
        return None;
    }
    let role_name = role.body.lines().next()?.trim().to_string();
    if role_name.is_empty() {
        return None;
    }
    // 3. Fetch the role's credentials JSON.
    let creds_req = HttpRequest {
        method: HttpMethod::Get,
        url: format!(
            "http://169.254.169.254/latest/meta-data/iam/security-credentials/{role_name}"
        ),
        headers: hdr,
        body: None,
        timeout: short,
    };
    let creds = transport.request(creds_req).await.ok()?;
    if creds.status != 200 {
        return None;
    }
    parse_json_credentials(&creds.body)
}

/// Run a `credential_process` command and parse its JSON output.
async fn from_credential_process(command: &str) -> Option<AwsCreds> {
    // The command is a shell-style argv; split on whitespace (sufficient for
    // the common `aws-vault exec …` / `aws configure export-credentials` forms).
    let mut parts = command.split_whitespace();
    let program = parts.next()?;
    let out = tokio::process::Command::new(program)
        .args(parts)
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_json_credentials(&String::from_utf8_lossy(&out.stdout))
}

/// Resolve AWS credentials via the layered chain. `transport` enables the IMDS
/// step (pass the registry's shared transport); `None` skips IMDS.
///
/// # Errors
/// Returns [`ApiError::Unauthorized`] when no source yields credentials.
pub async fn resolve(transport: Option<&Arc<dyn HttpTransport>>) -> Result<AwsCreds, ApiError> {
    if let Some(c) = from_env() {
        return Ok(c);
    }
    let profile = active_profile();
    if let Some(path) = shared_credentials_path() {
        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Some(c) = parse_ini_credentials(&content, &profile) {
                return Ok(c);
            }
        }
    }
    if let Some(path) = config_path() {
        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Some(cmd) = parse_credential_process(&content, &profile) {
                if let Some(c) = from_credential_process(&cmd).await {
                    return Ok(c);
                }
            }
        }
    }
    if let Some(t) = transport {
        if let Some(c) = from_imds(t).await {
            return Ok(c);
        }
    }
    Err(ApiError::Unauthorized(
        "no AWS credentials found (checked env, ~/.aws/credentials, credential_process, IMDS); \
         set AWS_ACCESS_KEY_ID/AWS_SECRET_ACCESS_KEY or run `aws sso login`"
            .to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ini_parses_named_profile_with_session_token() {
        let ini = "\
[default]
aws_access_key_id = AKIA_DEFAULT
aws_secret_access_key = secret_default

[work]
aws_access_key_id = AKIA_WORK
aws_secret_access_key = secret_work
aws_session_token = tok_work
";
        let d = parse_ini_credentials(ini, "default").unwrap();
        assert_eq!(d.access_key, "AKIA_DEFAULT");
        assert_eq!(d.session_token, None);
        let w = parse_ini_credentials(ini, "work").unwrap();
        assert_eq!(w.access_key, "AKIA_WORK");
        assert_eq!(w.session_token.as_deref(), Some("tok_work"));
        assert!(parse_ini_credentials(ini, "missing").is_none());
    }

    #[test]
    fn ini_handles_config_profile_prefix() {
        let cfg = "[profile work]\naws_access_key_id = A\naws_secret_access_key = B\n";
        let c = parse_ini_credentials(cfg, "work").unwrap();
        assert_eq!(c.access_key, "A");
        assert_eq!(c.secret_key, "B");
    }

    #[test]
    fn credential_process_extracted_for_profile() {
        let cfg = "\
[default]
region = us-east-1

[profile sso]
credential_process = aws-vault exec sso --json
";
        assert_eq!(
            parse_credential_process(cfg, "sso").as_deref(),
            Some("aws-vault exec sso --json")
        );
        assert!(parse_credential_process(cfg, "default").is_none());
    }

    #[test]
    fn json_creds_process_and_imds_shapes() {
        // credential_process schema (SessionToken).
        let proc = r#"{"Version":1,"AccessKeyId":"AKIA1","SecretAccessKey":"s1","SessionToken":"t1"}"#;
        let c = parse_json_credentials(proc).unwrap();
        assert_eq!(c.access_key, "AKIA1");
        assert_eq!(c.session_token.as_deref(), Some("t1"));
        // IMDS schema (Token).
        let imds = r#"{"AccessKeyId":"AKIA2","SecretAccessKey":"s2","Token":"t2","Expiration":"2026-06-01T12:00:00Z"}"#;
        let c2 = parse_json_credentials(imds).unwrap();
        assert_eq!(c2.access_key, "AKIA2");
        assert_eq!(c2.session_token.as_deref(), Some("t2"));
        // Missing secret → None.
        assert!(parse_json_credentials(r#"{"AccessKeyId":"x"}"#).is_none());
    }
}
