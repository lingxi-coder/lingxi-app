//! HTTP hook executor — POSTs the event JSON, parses the body.
//!
//! Drives `HookExecutor::Http` against an injected `Arc<dyn HttpTransport>`,
//! consulting the [`SsrfGuard`] before dispatch, applying a per-hook or
//! default timeout, and folding the response body through
//! [`crate::hook_payload::parse_response`].
//!
//! Signals back to the caller (in `executor.rs`) which arm-level telemetry
//! event the orchestrator should emit (`HOOK_HTTP_SKIPPED_SSRF` or
//! `HOOK_TIMEOUT`).
//!
//! Plan deviation: `HttpRequest` already carries an optional `timeout` field,
//! so we splice the effective timeout into the request rather than wrapping
//! the future in `tokio::time::timeout` (which would race two timeouts and
//! lose the structured `HttpError::Timeout`).

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use lingxi_protocol::{HttpMethod, HttpRequest};
use lingxi_traits::{HttpError, HttpTransport};

use crate::definition::{HookDefinition, HookExecutor};
use crate::hook_payload::parse_response;
use crate::response::{HookOutcome, HookResponse, HookResult};
use crate::ssrf_guard::SsrfGuard;

/// Telemetry hint the caller (`HookExecutorImpl::execute_single`) emits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HttpExecutionSignal {
    /// Request completed (success or non-SSRF non-timeout error).
    Ok,
    /// SSRF guard rejected the URL before dispatch.
    SsrfBlocked(String),
    /// Request exceeded the effective timeout.
    TimedOut,
}

/// Wrapper struct returned by [`HttpExecutor::execute`].
pub(crate) struct HttpExecutionOutcome {
    /// The raw `HookResult` to fold into `AggregateHookResult`.
    pub(crate) result: HookResult,
    /// Hint to the caller for arm-level telemetry emission.
    pub(crate) signal: HttpExecutionSignal,
}

pub(crate) struct HttpExecutor {
    pub(crate) http: Arc<dyn HttpTransport>,
    pub(crate) ssrf_guard: SsrfGuard,
    pub(crate) timeout: Duration,
}

impl HttpExecutor {
    /// Build a new executor with the supplied transport + SSRF policy.
    #[allow(dead_code)]
    pub(crate) fn new(
        http: Arc<dyn HttpTransport>,
        ssrf_guard: SsrfGuard,
        timeout: Duration,
    ) -> Self {
        Self {
            http,
            ssrf_guard,
            timeout,
        }
    }

    /// Execute one HTTP hook.
    ///
    /// `body` is the pre-serialized envelope JSON. `expected_event` is
    /// `"PreToolUse"` or `"PostToolUse"` and validates the nested
    /// `hookSpecificOutput.hookEventName` field per `claude-code`.
    pub(crate) async fn execute(
        &self,
        hook: &HookDefinition,
        url: &str,
        headers: &HashMap<String, String>,
        body: &str,
        expected_event: &'static str,
    ) -> HttpExecutionOutcome {
        // 1. SSRF check.
        if let Err(e) = self.ssrf_guard.check_url(url) {
            return HttpExecutionOutcome {
                result: HookResult {
                    outcome: HookOutcome::Error,
                    stdout: String::new(),
                    stderr: format!("Hook {} failed: SSRF guard rejected url: {}", hook.id, e),
                    exit_code: None,
                    response: None,
                },
                signal: HttpExecutionSignal::SsrfBlocked(e.to_string()),
            };
        }

        // 2. Pick effective timeout (per-hook override or default).
        let effective_timeout = match &hook.executor {
            HookExecutor::Http { timeout, .. } if !timeout.is_zero() => *timeout,
            _ => self.timeout,
        };

        // 3. Build the request. Inject Content-Type: application/json if the
        //    caller didn't supply one.
        let mut req_headers: Vec<(String, String)> = headers
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let has_content_type = req_headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("content-type"));
        if !has_content_type {
            req_headers.push(("Content-Type".into(), "application/json".into()));
        }
        let req = HttpRequest {
            method: HttpMethod::Post,
            url: url.to_string(),
            headers: req_headers,
            body: Some(body.to_string()),
            timeout: Some(effective_timeout),
        };

        // 4. Issue the request. `HttpTransport` enforces the request-level
        //    timeout natively and surfaces `HttpError::Timeout` on elapse.
        let raw = match self.http.request(req).await {
            Ok(r) => r,
            Err(HttpError::Timeout(_)) => {
                return HttpExecutionOutcome {
                    result: HookResult {
                        outcome: HookOutcome::Timeout,
                        stdout: String::new(),
                        stderr: format!(
                            "Hook {} failed: timeout after {}ms",
                            hook.id,
                            effective_timeout.as_millis()
                        ),
                        exit_code: None,
                        response: None,
                    },
                    signal: HttpExecutionSignal::TimedOut,
                };
            }
            Err(e) => {
                return HttpExecutionOutcome {
                    result: HookResult {
                        outcome: HookOutcome::Error,
                        stdout: String::new(),
                        stderr: format!("Hook {} failed: http error: {e}", hook.id),
                        exit_code: None,
                        response: None,
                    },
                    signal: HttpExecutionSignal::Ok,
                };
            }
        };

        // 5. Parse body if any. Even on non-2xx status, attempt to parse
        //    because some hooks return JSON + non-2xx to mean "advisory".
        let success = (200..300).contains(&raw.status);
        let parsed: Option<HookResponse> = if raw.body.is_empty() {
            None
        } else {
            parse_response(&raw.body, expected_event).ok()
        };

        HttpExecutionOutcome {
            result: HookResult {
                outcome: if success {
                    HookOutcome::Success
                } else {
                    HookOutcome::Error
                },
                stdout: raw.body,
                stderr: String::new(),
                exit_code: None,
                response: parsed,
            },
            signal: HttpExecutionSignal::Ok,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definition::{HookExecutor as DefHookExecutor, HookSource};
    use crate::events::HookEventType;
    use async_trait::async_trait;
    use lingxi_protocol::{HookId, HttpResponse};
    use std::sync::Mutex;

    struct MockHttp {
        recorded: Mutex<Vec<HttpRequest>>,
        response_body: String,
        response_status: u16,
        error_to_return: Mutex<Option<HttpError>>,
    }

    #[async_trait]
    impl HttpTransport for MockHttp {
        async fn request(&self, req: HttpRequest) -> Result<HttpResponse, HttpError> {
            if let Some(e) = self.error_to_return.lock().unwrap().take() {
                return Err(e);
            }
            self.recorded.lock().unwrap().push(req);
            Ok(HttpResponse {
                status: self.response_status,
                headers: Vec::new(),
                body: self.response_body.clone(),
            })
        }
        async fn stream_sse(
            &self,
            _req: HttpRequest,
        ) -> Result<lingxi_traits::http::SseStream, HttpError> {
            Err(HttpError::InvalidRequest("not implemented".into()))
        }
    }

    fn make_http_hook(url: &str) -> HookDefinition {
        HookDefinition {
            id: HookId::new(),
            name: "test-http".into(),
            events: vec![HookEventType::PreToolUse],
            if_condition: None,
            executor: DefHookExecutor::Http {
                url: url.into(),
                method: "POST".into(),
                headers: HashMap::new(),
                timeout: Duration::from_secs(5),
            },
            source: HookSource::User,
            blocking: true,
            timeout: None,
            priority: 0,
        }
    }

    #[tokio::test]
    async fn returns_approve_when_endpoint_responds_with_allow() {
        let http = Arc::new(MockHttp {
            recorded: Mutex::new(Vec::new()),
            response_status: 200,
            response_body:
                r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow"}}"#
                    .into(),
            error_to_return: Mutex::new(None),
        });
        let exec = HttpExecutor::new(
            http.clone(),
            SsrfGuard::with_defaults(),
            Duration::from_secs(5),
        );
        let hook = make_http_hook("https://hook.example.com/pre");

        let outcome = exec
            .execute(
                &hook,
                "https://hook.example.com/pre",
                &HashMap::new(),
                "{}",
                "PreToolUse",
            )
            .await;

        assert_eq!(outcome.signal, HttpExecutionSignal::Ok);
        assert!(matches!(outcome.result.outcome, HookOutcome::Success));
        let resp = outcome.result.response.expect("response parsed");
        assert_eq!(resp.decision, Some(crate::response::HookDecision::Approve));
        assert_eq!(http.recorded.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn ssrf_blocks_link_local() {
        let http = Arc::new(MockHttp {
            recorded: Mutex::new(Vec::new()),
            response_status: 200,
            response_body: String::new(),
            error_to_return: Mutex::new(None),
        });
        let exec = HttpExecutor::new(
            http.clone(),
            SsrfGuard::with_defaults(),
            Duration::from_secs(5),
        );
        let hook = make_http_hook("http://169.254.169.254/meta");

        let outcome = exec
            .execute(
                &hook,
                "http://169.254.169.254/meta",
                &HashMap::new(),
                "{}",
                "PreToolUse",
            )
            .await;

        assert!(matches!(
            outcome.signal,
            HttpExecutionSignal::SsrfBlocked(_)
        ));
        assert!(matches!(outcome.result.outcome, HookOutcome::Error));
        assert_eq!(
            http.recorded.lock().unwrap().len(),
            0,
            "no request should be sent"
        );
    }

    #[tokio::test]
    async fn timeout_surfaces_as_timed_out() {
        let http = Arc::new(MockHttp {
            recorded: Mutex::new(Vec::new()),
            response_status: 200,
            response_body: String::new(),
            error_to_return: Mutex::new(Some(HttpError::Timeout(Duration::from_millis(1)))),
        });
        let exec = HttpExecutor::new(
            http.clone(),
            SsrfGuard::with_defaults(),
            Duration::from_millis(1),
        );
        let hook = make_http_hook("https://hook.example.com/pre");

        let outcome = exec
            .execute(
                &hook,
                "https://hook.example.com/pre",
                &HashMap::new(),
                "{}",
                "PreToolUse",
            )
            .await;

        assert_eq!(outcome.signal, HttpExecutionSignal::TimedOut);
        assert!(matches!(outcome.result.outcome, HookOutcome::Timeout));
    }
}
