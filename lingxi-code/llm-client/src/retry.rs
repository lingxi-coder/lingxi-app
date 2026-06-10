//! Retry classification over provider-neutral errors and response metadata.

use std::{collections::BTreeMap, time::Duration};

use crate::LlmError;

/// Retry policy for response and error classification.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RetryPolicy;

impl RetryPolicy {
    /// Classify an error into a retry decision.
    #[must_use]
    pub fn classify_error(&self, error: &LlmError) -> RetryDecision {
        match error {
            LlmError::Transport { .. } | LlmError::ProviderInternal | LlmError::Overloaded => {
                RetryDecision::Retry { after: None }
            }
            LlmError::RateLimited { retry_after, .. } => RetryDecision::Retry {
                after: *retry_after,
            },
            LlmError::Authentication
            | LlmError::PermissionDenied
            | LlmError::InvalidRequest { .. }
            | LlmError::QuotaExceeded
            | LlmError::ContextOverflow
            | LlmError::ModelUnavailable
            | LlmError::StreamInterrupted { .. }
            | LlmError::CostUnavailable { .. }
            | LlmError::UnsupportedCapability { .. } => RetryDecision::DoNotRetry,
        }
    }

    /// Classify raw response metadata into a retry decision.
    #[must_use]
    pub fn classify_response(&self, metadata: &ResponseMetadata) -> RetryDecision {
        if matches!(metadata.status, 429 | 500 | 502 | 503 | 504 | 529) {
            RetryDecision::Retry {
                after: metadata.retry_after(),
            }
        } else {
            RetryDecision::DoNotRetry
        }
    }
}

/// Retry classifier output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryDecision {
    /// Retry the request, optionally after a server-provided duration.
    Retry {
        /// Delay before retrying.
        after: Option<Duration>,
    },
    /// Do not retry the request.
    DoNotRetry,
}

/// Response metadata needed for retry classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseMetadata {
    /// HTTP status code.
    pub status: u16,
    /// Lowercase response headers.
    pub headers: BTreeMap<String, String>,
}

impl ResponseMetadata {
    /// Create response metadata with no headers.
    #[must_use]
    pub fn new(status: u16) -> Self {
        Self {
            status,
            headers: BTreeMap::new(),
        }
    }

    /// Add a header to metadata.
    #[must_use]
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers
            .insert(name.into().to_ascii_lowercase(), value.into());
        self
    }

    fn retry_after(&self) -> Option<Duration> {
        retry_after_from_headers(&self.headers)
    }
}

/// Parse a retry delay from response headers.
///
/// Prefers the millisecond-precision `retry-after-ms` header over the
/// standard `retry-after` seconds form. Names match case-insensitively so
/// non-normalized header maps work too.
pub(crate) fn retry_after_from_headers(headers: &BTreeMap<String, String>) -> Option<Duration> {
    if let Some(value) = header_value(headers, "retry-after-ms") {
        if let Ok(milliseconds) = value.trim().parse::<u64>() {
            return Some(Duration::from_millis(milliseconds));
        }
    }
    header_value(headers, "retry-after")
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
}

fn header_value<'headers>(
    headers: &'headers BTreeMap<String, String>,
    name: &str,
) -> Option<&'headers str> {
    headers.get(name).map(String::as_str).or_else(|| {
        headers
            .iter()
            .find_map(|(key, value)| key.eq_ignore_ascii_case(name).then_some(value.as_str()))
    })
}
