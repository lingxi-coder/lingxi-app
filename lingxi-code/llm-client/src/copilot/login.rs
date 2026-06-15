//! GitHub Copilot device-flow login (RFC 8628) over an injected JSON-POST seam.
//!
//! The crate owns no HTTP client (transport is always injected). [`CopilotLogin`]
//! holds the pure begin/poll state machine + backoff math; the host implements
//! [`CopilotHttp`] and drives the sleep/retry loop and any UI.

use serde_json::{json, Value};

use crate::copilot::auth::CopilotSecret;
use crate::transport::BoxFuture;
use crate::LlmError;

/// GitHub OAuth App client id (opencode's public Copilot app).
pub const COPILOT_CLIENT_ID: &str = "Ov23li8tweQw6odWQebz";

const DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
const ACCESS_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";

/// Minimal JSON-POST seam for the two device-flow calls.
///
/// Host implementations MUST send `Accept: application/json` (GitHub otherwise
/// form-encodes the response) and a `User-Agent`. Transport/TLS/timeout failures
/// map to [`LlmError::Transport`].
pub trait CopilotHttp: Send + Sync {
    /// POST `url` with a JSON body; return the parsed JSON response.
    fn post_json<'a>(
        &'a self,
        url: &'a str,
        body: &'a Value,
    ) -> BoxFuture<'a, Result<Value, LlmError>>;
}

/// Device-code grant returned by [`CopilotLogin::begin`].
#[derive(Debug, Clone)]
pub struct DeviceCodeResponse {
    /// Code the user types at `verification_uri`.
    pub user_code: String,
    /// URL the user opens to authorize.
    pub verification_uri: String,
    /// Opaque device code used when polling.
    pub device_code: String,
    /// Server-recommended polling interval (seconds).
    pub interval_secs: u64,
}

/// Classified result of one [`CopilotLogin::poll_once`].
#[derive(Debug)]
pub enum PollOutcome {
    /// Authorization complete; carries the GitHub OAuth token.
    Success(CopilotSecret),
    /// Not authorized yet; sleep `interval_secs` and poll again.
    Pending {
        /// Seconds to wait before the next poll.
        interval_secs: u64,
    },
    /// Server asked us to slow down; sleep `interval_secs` and poll again.
    SlowDown {
        /// Backed-off seconds to wait (RFC 8628 §3.5).
        interval_secs: u64,
    },
    /// Terminal failure (e.g. `access_denied`, `expired_token`).
    Failed {
        /// Server-reported error code.
        error: String,
    },
}

/// Device-flow login driver. The host owns the sleep+retry loop.
pub struct CopilotLogin<H: CopilotHttp> {
    http: H,
    client_id: String,
}

impl<H: CopilotHttp> CopilotLogin<H> {
    /// Create a login driver over the given HTTP seam.
    #[must_use]
    pub fn new(http: H) -> Self {
        Self {
            http,
            client_id: COPILOT_CLIENT_ID.to_string(),
        }
    }

    /// Step 1 — request a device code. The caller displays `user_code` +
    /// `verification_uri`, then polls.
    pub async fn begin(&self) -> Result<DeviceCodeResponse, LlmError> {
        let body = json!({ "client_id": self.client_id, "scope": "read:user" });
        let v = self.http.post_json(DEVICE_CODE_URL, &body).await?;
        Ok(DeviceCodeResponse {
            user_code: str_field(&v, "user_code")?,
            verification_uri: str_field(&v, "verification_uri")?,
            device_code: str_field(&v, "device_code")?,
            interval_secs: v.get("interval").and_then(Value::as_u64).unwrap_or(5),
        })
    }

    /// Step 2 — poll once for the token. Returns a classified [`PollOutcome`]
    /// with the recommended next interval; the caller sleeps and re-polls on
    /// `Pending`/`SlowDown`.
    pub async fn poll_once(&self, dc: &DeviceCodeResponse) -> Result<PollOutcome, LlmError> {
        let body = json!({
            "client_id": self.client_id,
            "device_code": dc.device_code,
            "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
        });
        let v = self.http.post_json(ACCESS_TOKEN_URL, &body).await?;

        if let Some(token) = v.get("access_token").and_then(Value::as_str) {
            return Ok(PollOutcome::Success(CopilotSecret::new(token)));
        }
        match v.get("error").and_then(Value::as_str) {
            Some("authorization_pending") => Ok(PollOutcome::Pending {
                interval_secs: dc.interval_secs,
            }),
            Some("slow_down") => {
                // RFC 8628 §3.5: add 5s, or use the server-provided interval.
                let server = v.get("interval").and_then(Value::as_u64);
                Ok(PollOutcome::SlowDown {
                    interval_secs: server.unwrap_or(dc.interval_secs + 5),
                })
            }
            Some(other) => Ok(PollOutcome::Failed {
                error: other.to_string(),
            }),
            None => Ok(PollOutcome::Failed {
                error: "no access_token and no error in response".to_string(),
            }),
        }
    }
}

fn str_field(v: &Value, key: &str) -> Result<String, LlmError> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| LlmError::InvalidRequest {
            message: format!("copilot device-flow response missing '{key}'"),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockHttp(Value);
    impl CopilotHttp for MockHttp {
        fn post_json<'a>(
            &'a self,
            _url: &'a str,
            _body: &'a Value,
        ) -> BoxFuture<'a, Result<Value, LlmError>> {
            let v = self.0.clone();
            Box::pin(async move { Ok(v) })
        }
    }

    fn dc() -> DeviceCodeResponse {
        DeviceCodeResponse {
            user_code: "WDJB-MJHT".to_string(),
            verification_uri: "https://github.com/login/device".to_string(),
            device_code: "dev-code".to_string(),
            interval_secs: 5,
        }
    }

    #[tokio::test]
    async fn begin_parses_device_code() {
        let login = CopilotLogin::new(MockHttp(json!({
            "user_code": "WDJB-MJHT",
            "verification_uri": "https://github.com/login/device",
            "device_code": "dev-code",
            "interval": 7
        })));
        let parsed = login.begin().await.expect("begin ok");
        assert_eq!(parsed.user_code, "WDJB-MJHT");
        assert_eq!(parsed.device_code, "dev-code");
        assert_eq!(parsed.interval_secs, 7);
    }

    #[tokio::test]
    async fn poll_success_yields_token() {
        let login = CopilotLogin::new(MockHttp(json!({ "access_token": "ght_abc" })));
        match login.poll_once(&dc()).await.expect("poll ok") {
            PollOutcome::Success(secret) => {
                assert!(!format!("{secret:?}").contains("ght_abc"));
            }
            other => panic!("expected Success, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn poll_pending_returns_device_interval() {
        let login = CopilotLogin::new(MockHttp(json!({ "error": "authorization_pending" })));
        match login.poll_once(&dc()).await.unwrap() {
            PollOutcome::Pending { interval_secs } => assert_eq!(interval_secs, 5),
            other => panic!("expected Pending, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn poll_slow_down_without_server_interval_adds_five() {
        let login = CopilotLogin::new(MockHttp(json!({ "error": "slow_down" })));
        match login.poll_once(&dc()).await.unwrap() {
            PollOutcome::SlowDown { interval_secs } => assert_eq!(interval_secs, 10),
            other => panic!("expected SlowDown, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn poll_slow_down_uses_server_interval_when_present() {
        let login = CopilotLogin::new(MockHttp(json!({ "error": "slow_down", "interval": 42 })));
        match login.poll_once(&dc()).await.unwrap() {
            PollOutcome::SlowDown { interval_secs } => assert_eq!(interval_secs, 42),
            other => panic!("expected SlowDown, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn poll_other_error_is_terminal() {
        let login = CopilotLogin::new(MockHttp(json!({ "error": "access_denied" })));
        match login.poll_once(&dc()).await.unwrap() {
            PollOutcome::Failed { error } => assert_eq!(error, "access_denied"),
            other => panic!("expected Failed, got {other:?}"),
        }
    }
}
