//! Async, request-aware auth seam. `Authenticator::authorize` mutates a built
//! `HttpRequest` immediately before transport — the point where AWS `SigV4` (which
//! signs over method + URI + headers + body + timestamp) and async cloud-token
//! minting (Vertex / Azure AD) must run. `StaticAuth` wraps the synchronous
//! [`Auth`] header styles (API keys); signed authenticators land in later phases.

use std::sync::Arc;
use std::time::Duration;

use aws_credential_types::Credentials;
use aws_sigv4::http_request::{sign, SignableBody, SignableRequest, SigningSettings};
use aws_sigv4::sign::v4;

use crate::auth::Auth;
use crate::aws_creds::AwsCreds;
use api_client::ApiError;
use async_trait::async_trait;
use protocol::HttpRequest;
use traits::HttpTransport;

/// Attaches authentication to a fully-built request immediately before it is
/// sent. Object-safe so `GenericClient` can hold an `Arc<dyn Authenticator>`.
#[async_trait]
pub trait Authenticator: Send + Sync {
    /// Attach or replace auth on `req` (headers, or a signed `Authorization`).
    ///
    /// # Errors
    /// Returns [`ApiError`] if credentials cannot be resolved or signing fails.
    async fn authorize(&self, req: &mut HttpRequest) -> Result<(), ApiError>;
}

/// An [`Authenticator`] for static API-key header styles — wraps the v1 [`Auth`]
/// enum (`None` / `Bearer` / `Header`). Performs no I/O; the async signature is
/// uniform with the signed authenticators added later.
#[derive(Debug, Clone)]
pub struct StaticAuth(Auth);

impl StaticAuth {
    /// Wrap an [`Auth`] header style as an [`Authenticator`].
    #[must_use]
    pub fn new(auth: Auth) -> Self {
        Self(auth)
    }
}

#[async_trait]
impl Authenticator for StaticAuth {
    async fn authorize(&self, req: &mut HttpRequest) -> Result<(), ApiError> {
        self.0.apply(&mut req.headers);
        Ok(())
    }
}

/// Mints + caches a GCP `OAuth2` access token via `gcp_auth` and attaches it as
/// `Authorization: Bearer …`. Used by Vertex AI.
///
/// `gcp_auth` discovers credentials in this order:
/// 1. `GOOGLE_APPLICATION_CREDENTIALS` → a service-account key file.
/// 2. Application Default Credentials (`gcloud auth application-default login`,
///    i.e. `~/.config/gcloud/application_default_credentials.json`).
/// 3. The GCE / Cloud Run metadata server (the attached service account).
///
/// The token provider is created lazily on first use (the registry builds
/// synchronously; `gcp_auth` provider discovery is async).
#[derive(Default)]
pub struct GcpTokenAuthenticator {
    provider: tokio::sync::OnceCell<Arc<dyn gcp_auth::TokenProvider>>,
}

impl GcpTokenAuthenticator {
    /// Construct an uninitialized authenticator (provider discovered on first use).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

/// GCP scope granting access to Vertex AI (and other cloud APIs).
const GCP_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";

#[async_trait]
impl Authenticator for GcpTokenAuthenticator {
    async fn authorize(&self, req: &mut HttpRequest) -> Result<(), ApiError> {
        let provider = self
            .provider
            .get_or_try_init(gcp_auth::provider)
            .await
            .map_err(|e| {
                ApiError::Unauthorized(format!(
                    "GCP auth: no credentials found — set GOOGLE_APPLICATION_CREDENTIALS to a \
                     service-account key file, or run `gcloud auth application-default login` \
                     ({e})"
                ))
            })?;
        let token = provider
            .token(&[GCP_SCOPE])
            .await
            .map_err(|e| ApiError::Unauthorized(format!("GCP auth: token request failed: {e}")))?;
        req.headers
            .push(("authorization".to_string(), format!("Bearer {}", token.as_str())));
        Ok(())
    }
}

/// Signs Bedrock requests with AWS `SigV4`. Credentials come from the layered
/// discovery chain in [`crate::aws_creds`] (env → shared credentials file →
/// `credential_process` → `IMDSv2`), resolved lazily and cached for
/// [`CRED_TTL`]. The IMDS step is only attempted when a transport is supplied
/// via [`SigV4Authenticator::with_transport`].
pub struct SigV4Authenticator {
    region: String,
    /// Shared transport used for the `IMDSv2` credential fetch (None → IMDS skipped).
    transport: Option<Arc<dyn HttpTransport>>,
    /// Cached resolved credentials + the instant they were resolved.
    cache: tokio::sync::Mutex<Option<(AwsCreds, std::time::Instant)>>,
}

/// How long resolved credentials are cached before re-resolution. Bounds both
/// env/file re-reads and IMDS round-trips while staying well inside the
/// ~6h lifetime of IMDS-issued credentials.
const CRED_TTL: Duration = Duration::from_secs(300);

impl SigV4Authenticator {
    /// Construct for an AWS region (e.g. `us-east-1`) with no transport — the
    /// IMDS step is skipped (env / credentials file / `credential_process` only).
    #[must_use]
    pub fn new(region: String) -> Self {
        Self {
            region,
            transport: None,
            cache: tokio::sync::Mutex::new(None),
        }
    }

    /// Construct with a shared transport so the `IMDSv2` credential source is
    /// available (for EC2/ECS instance-role credentials).
    #[must_use]
    pub fn with_transport(region: String, transport: Arc<dyn HttpTransport>) -> Self {
        Self {
            region,
            transport: Some(transport),
            cache: tokio::sync::Mutex::new(None),
        }
    }

    /// Resolve credentials via the discovery chain, caching for [`CRED_TTL`].
    async fn resolve_cached(&self) -> Result<AwsCreds, ApiError> {
        let mut guard = self.cache.lock().await;
        if let Some((creds, at)) = guard.as_ref() {
            if at.elapsed() < CRED_TTL {
                return Ok(creds.clone());
            }
        }
        let fresh = crate::aws_creds::resolve(self.transport.as_ref()).await?;
        *guard = Some((fresh.clone(), std::time::Instant::now()));
        Ok(fresh)
    }

    /// Sign `req` in-place using the supplied credentials and timestamp.
    ///
    /// Extracted from [`Authenticator::authorize`] so it can be called with fixed
    /// inputs in tests (no env vars, no `SystemTime::now()` non-determinism).
    ///
    /// # Errors
    /// Returns [`ApiError::Unauthorized`] if `SigV4` signing fails.
    fn sign_in_place(
        &self,
        req: &mut HttpRequest,
        access_key: &str,
        secret_key: &str,
        session_token: Option<&str>,
        time: std::time::SystemTime,
    ) -> Result<(), ApiError> {
        let creds = Credentials::new(
            access_key,
            secret_key,
            session_token.map(str::to_owned),
            None,
            "bedrock-env",
        );
        let identity = creds.into();

        let settings = SigningSettings::default();
        let signing_params = v4::SigningParams::builder()
            .identity(&identity)
            .region(self.region.as_str())
            .name("bedrock")
            .time(time)
            .settings(settings)
            .build()
            .map_err(|e| ApiError::Unauthorized(format!("SigV4: failed to build signing params: {e}")))?
            .into();

        let method_str = match req.method {
            protocol::HttpMethod::Post => "POST",
            protocol::HttpMethod::Get => "GET",
            protocol::HttpMethod::Put => "PUT",
            protocol::HttpMethod::Patch => "PATCH",
            protocol::HttpMethod::Delete => "DELETE",
            protocol::HttpMethod::Head => "HEAD",
            protocol::HttpMethod::Options => "OPTIONS",
        };
        let body_bytes: &[u8] = req.body.as_deref().map_or(&[], str::as_bytes);

        // Headers must be borrowed from `req`; collect refs before calling sign.
        let header_pairs: Vec<(&str, &str)> = req
            .headers
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        let signable = SignableRequest::new(
            method_str,
            &req.url,
            header_pairs.iter().copied(),
            SignableBody::Bytes(body_bytes),
        )
        .map_err(|e| ApiError::Unauthorized(format!("SigV4: failed to build signable request: {e}")))?;

        let (instructions, _sig) = sign(signable, &signing_params)
            .map_err(|e| ApiError::Unauthorized(format!("SigV4: signing failed: {e}")))?
            .into_parts();

        for (name, value) in instructions.headers() {
            req.headers.push((name.to_string(), value.to_string()));
        }

        Ok(())
    }
}

#[async_trait]
impl Authenticator for SigV4Authenticator {
    async fn authorize(&self, req: &mut HttpRequest) -> Result<(), ApiError> {
        let creds = self.resolve_cached().await?;
        self.sign_in_place(
            req,
            &creds.access_key,
            &creds.secret_key,
            creds.session_token.as_deref(),
            std::time::SystemTime::now(),
        )
    }
}

/// Default Azure `OpenAI` token scope for client-credentials grants.
const AZURE_DEFAULT_SCOPE: &str = "https://cognitiveservices.azure.com/.default";

/// Percent-encode a value for `application/x-www-form-urlencoded` (RFC 3986
/// unreserved set passes through; everything else is `%XX`).
fn pct_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Build a form-urlencoded body from key/value pairs.
fn form_encode(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", pct_encode(k), pct_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Parse `access_token` + `expires_in` (seconds) from an Azure AD token response.
fn parse_token_response(json: &str) -> Option<(String, u64)> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let token = v.get("access_token")?.as_str()?.to_string();
    let expires_in = v
        .get("expires_in")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(3600);
    Some((token, expires_in))
}

/// Mints + caches an Azure AD (Entra ID) bearer token via the `OAuth2`
/// client-credentials grant, attaching it as `Authorization: Bearer …`. Used by
/// Azure `OpenAI` profiles that configure `azureAd` instead of an `apiKeyEnv`.
pub struct AzureAdAuthenticator {
    tenant: String,
    client_id: String,
    client_secret: String,
    scope: String,
    transport: Arc<dyn HttpTransport>,
    /// Cached `(token, expiry_instant)`.
    cache: tokio::sync::Mutex<Option<(String, std::time::Instant)>>,
}

impl AzureAdAuthenticator {
    /// Construct from a tenant id, resolved client id/secret, and a transport.
    #[must_use]
    pub fn new(
        tenant: String,
        client_id: String,
        client_secret: String,
        transport: Arc<dyn HttpTransport>,
    ) -> Self {
        Self {
            tenant,
            client_id,
            client_secret,
            scope: AZURE_DEFAULT_SCOPE.to_string(),
            transport,
            cache: tokio::sync::Mutex::new(None),
        }
    }

    async fn token(&self) -> Result<String, ApiError> {
        let mut guard = self.cache.lock().await;
        if let Some((tok, exp)) = guard.as_ref() {
            if std::time::Instant::now() < *exp {
                return Ok(tok.clone());
            }
        }
        let body = form_encode(&[
            ("grant_type", "client_credentials"),
            ("client_id", &self.client_id),
            ("client_secret", &self.client_secret),
            ("scope", &self.scope),
        ]);
        let req = HttpRequest {
            method: protocol::HttpMethod::Post,
            url: format!(
                "https://login.microsoftonline.com/{}/oauth2/v2.0/token",
                self.tenant
            ),
            headers: vec![(
                "content-type".to_string(),
                "application/x-www-form-urlencoded".to_string(),
            )],
            body: Some(body),
            timeout: Some(Duration::from_secs(30)),
        };
        let resp = self
            .transport
            .request(req)
            .await
            .map_err(|e| ApiError::Unauthorized(format!("Azure AD: token request failed: {e}")))?;
        if resp.status != 200 {
            return Err(ApiError::Unauthorized(format!(
                "Azure AD: token endpoint returned {} ({})",
                resp.status, resp.body
            )));
        }
        let (token, expires_in) = parse_token_response(&resp.body).ok_or_else(|| {
            ApiError::Unauthorized("Azure AD: malformed token response".to_string())
        })?;
        let exp =
            std::time::Instant::now() + Duration::from_secs(expires_in.saturating_sub(60).max(1));
        *guard = Some((token.clone(), exp));
        Ok(token)
    }
}

#[async_trait]
impl Authenticator for AzureAdAuthenticator {
    async fn authorize(&self, req: &mut HttpRequest) -> Result<(), ApiError> {
        let token = self.token().await?;
        req.headers
            .push(("authorization".to_string(), format!("Bearer {token}")));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::HttpMethod;

    fn req() -> HttpRequest {
        HttpRequest {
            method: HttpMethod::Post,
            url: "https://x.local/v1".to_string(),
            headers: vec![("content-type".to_string(), "application/json".to_string())],
            body: Some("{}".to_string()),
            timeout: None,
        }
    }

    #[tokio::test]
    async fn static_bearer_attaches_authorization() {
        let a = StaticAuth::new(Auth::Bearer("sk-1".to_string()));
        let mut r = req();
        a.authorize(&mut r).await.unwrap();
        assert!(r
            .headers
            .iter()
            .any(|(k, v)| k == "authorization" && v == "Bearer sk-1"));
    }

    #[tokio::test]
    async fn static_header_attaches_named_header() {
        let a = StaticAuth::new(Auth::Header {
            name: "x-goog-api-key".to_string(),
            value: "k".to_string(),
        });
        let mut r = req();
        a.authorize(&mut r).await.unwrap();
        assert!(r.headers.iter().any(|(k, v)| k == "x-goog-api-key" && v == "k"));
    }

    #[tokio::test]
    async fn static_none_adds_no_header() {
        let a = StaticAuth::new(Auth::None);
        let mut r = req();
        let before = r.headers.len();
        a.authorize(&mut r).await.unwrap();
        assert_eq!(r.headers.len(), before);
    }

    #[test]
    fn gcp_authenticator_constructs() {
        let _a = GcpTokenAuthenticator::new();
        // authorize() needs live GCP credentials, so it is not exercised here.
    }

    #[test]
    fn azure_ad_form_encode_and_token_parse() {
        let body = form_encode(&[
            ("grant_type", "client_credentials"),
            ("scope", "https://cognitiveservices.azure.com/.default"),
        ]);
        assert!(body.contains("grant_type=client_credentials"));
        // reserved chars in the scope are percent-encoded
        assert!(body.contains("scope=https%3A%2F%2Fcognitiveservices.azure.com%2F.default"));
        let (tok, exp) =
            parse_token_response(r#"{"access_token":"abc","expires_in":3599,"token_type":"Bearer"}"#)
                .unwrap();
        assert_eq!(tok, "abc");
        assert_eq!(exp, 3599);
        // missing access_token → None
        assert!(parse_token_response(r#"{"error":"invalid_client"}"#).is_none());
    }

    #[test]
    fn sigv4_signs_with_authorization_header_and_is_deterministic() {
        let auth = SigV4Authenticator::new("us-east-1".to_string());
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_440_938_160);
        let mk = || HttpRequest {
            method: protocol::HttpMethod::Post,
            url: "https://bedrock-runtime.us-east-1.amazonaws.com/model/m/invoke".to_string(),
            headers: vec![("content-type".to_string(), "application/json".to_string())],
            body: Some("{}".to_string()),
            timeout: None,
        };
        let mut a = mk();
        auth.sign_in_place(&mut a, "AKIDEXAMPLE", "secret", None, t).unwrap();
        let (_, v) = a
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
            .expect("authz header");
        assert!(v.starts_with("AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/"));
        assert!(v.contains("/us-east-1/bedrock/aws4_request"));
        let mut b = mk();
        auth.sign_in_place(&mut b, "AKIDEXAMPLE", "secret", None, t).unwrap();
        assert_eq!(a.headers, b.headers); // deterministic
    }
}
