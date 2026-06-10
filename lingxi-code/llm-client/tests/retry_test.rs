use std::time::Duration;

use llm_client::{LlmError, ResponseMetadata, RetryDecision, RetryPolicy};

#[test]
fn retryable_response_statuses_are_retried() {
    let policy = RetryPolicy;

    for status in [429, 500, 502, 503, 504, 529] {
        let decision = policy.classify_response(&ResponseMetadata::new(status));
        assert_eq!(decision, RetryDecision::Retry { after: None }, "status {status}");
    }
}

#[test]
fn retry_after_header_seconds_are_preserved() {
    let metadata = ResponseMetadata::new(429).with_header("retry-after", "7");

    let decision = RetryPolicy.classify_response(&metadata);

    assert_eq!(
        decision,
        RetryDecision::Retry {
            after: Some(Duration::from_secs(7))
        }
    );
}

#[test]
fn retry_after_ms_header_takes_precedence_over_seconds() {
    let metadata = ResponseMetadata::new(429)
        .with_header("retry-after", "7")
        .with_header("retry-after-ms", "250");

    let decision = RetryPolicy.classify_response(&metadata);

    assert_eq!(
        decision,
        RetryDecision::Retry {
            after: Some(Duration::from_millis(250))
        }
    );
}

#[test]
fn non_retryable_errors_do_not_retry() {
    let policy = RetryPolicy;
    let errors = [
        LlmError::Authentication,
        LlmError::PermissionDenied,
        LlmError::InvalidRequest {
            message: "bad".to_string(),
        },
        LlmError::ContextOverflow,
        LlmError::UnsupportedCapability {
            capability: "vision".to_string(),
        },
        LlmError::CostUnavailable {
            message: "missing".to_string(),
        },
    ];

    for error in errors {
        assert_eq!(policy.classify_error(&error), RetryDecision::DoNotRetry);
    }
}

#[test]
fn stream_interruption_after_events_is_not_replayed() {
    let error = LlmError::StreamInterrupted {
        message: "socket closed".to_string(),
    };

    assert_eq!(RetryPolicy.classify_error(&error), RetryDecision::DoNotRetry);
}

#[test]
fn overloaded_errors_are_retryable() {
    assert_eq!(
        RetryPolicy.classify_error(&LlmError::Overloaded),
        RetryDecision::Retry { after: None }
    );
}
