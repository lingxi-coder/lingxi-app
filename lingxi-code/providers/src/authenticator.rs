//! Async, request-aware auth seam. `Authenticator::authorize` mutates a built
//! `HttpRequest` immediately before transport — the point where AWS `SigV4` (which
//! signs over method + URI + headers + body + timestamp) and async cloud-token
//! minting (Vertex / Azure AD) must run. `StaticAuth` wraps the synchronous
//! [`Auth`] header styles (API keys); signed authenticators land in later phases.

use crate::auth::Auth;
use api_client::ApiError;
use async_trait::async_trait;
use protocol::HttpRequest;

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
pub struct StaticAuth(pub Auth);

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
}
