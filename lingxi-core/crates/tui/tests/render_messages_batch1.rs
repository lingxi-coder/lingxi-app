//! M7-04 batch-1 renderer snapshots.
use lingxi_tui::components::messages::advisor::{render_advisor_to_string, AdvisorProps};
use lingxi_tui::components::messages::compact_boundary::render_compact_boundary_to_string;
use lingxi_tui::components::messages::rate_limit::{render_rate_limit_to_string, RateLimitProps};
use lingxi_tui::components::messages::redacted_thinking::render_redacted_thinking_to_string;
use lingxi_tui::components::messages::shutdown::{render_shutdown_to_string, ShutdownProps};
use lingxi_tui::components::messages::system_api_error::{
    render_system_api_error_to_string, SystemApiErrorProps,
};
use lingxi_tui::components::messages::system_text::{render_system_text_to_string, SystemTextProps};
use lingxi_tui::components::messages::thinking::{render_thinking_to_string, ThinkingProps};
use lingxi_tui::state::{AdvisorKind, SystemLevel};

#[test]
fn thinking_collapsed() {
    let s = render_thinking_to_string(ThinkingProps {
        thinking: "Considering the tradeoffs between A and B.".into(),
        expanded: false,
    });
    insta::assert_snapshot!("thinking_collapsed", s);
}

#[test]
fn thinking_expanded() {
    let s = render_thinking_to_string(ThinkingProps {
        thinking: "Step one.\nStep two.".into(),
        expanded: true,
    });
    insta::assert_snapshot!("thinking_expanded", s);
}

#[test]
fn redacted_thinking_line() {
    insta::assert_snapshot!(
        "redacted_thinking_line",
        render_redacted_thinking_to_string()
    );
}

#[test]
fn compact_boundary_line() {
    insta::assert_snapshot!("compact_boundary_line", render_compact_boundary_to_string());
}

#[test]
fn system_text_info_plain() {
    let s = render_system_text_to_string(SystemTextProps {
        body: "Saved settings.".into(),
        level: SystemLevel::Info,
    });
    insta::assert_snapshot!("system_text_info_plain", s);
}

#[test]
fn system_text_warning_dotted() {
    let s = render_system_text_to_string(SystemTextProps {
        body: "Approaching context limit.".into(),
        level: SystemLevel::Warning,
    });
    insta::assert_snapshot!("system_text_warning_dotted", s);
}

#[test]
fn system_api_error_with_retry() {
    let s = render_system_api_error_to_string(SystemApiErrorProps {
        error: "529 Overloaded".into(),
        retry_attempt: 4,
        retry_in_seconds: 3,
        max_retries: 10,
        truncated: false,
    });
    insta::assert_snapshot!("system_api_error_with_retry", s);
}

#[test]
fn system_api_error_singular_second_and_truncated() {
    let s = render_system_api_error_to_string(SystemApiErrorProps {
        error: "boom".into(),
        retry_attempt: 5,
        retry_in_seconds: 1,
        max_retries: 10,
        truncated: true,
    });
    insta::assert_snapshot!("system_api_error_singular_second_and_truncated", s);
}

#[test]
fn rate_limit_with_upsell() {
    let s = render_rate_limit_to_string(RateLimitProps {
        text: "You've hit your usage limit.".into(),
        upsell: Some("/upgrade to increase your usage limit.".into()),
    });
    insta::assert_snapshot!("rate_limit_with_upsell", s);
}

#[test]
fn rate_limit_no_upsell() {
    let s = render_rate_limit_to_string(RateLimitProps {
        text: "You've hit your usage limit.".into(),
        upsell: None,
    });
    insta::assert_snapshot!("rate_limit_no_upsell", s);
}

#[test]
fn shutdown_request_with_reason() {
    let s = render_shutdown_to_string(ShutdownProps {
        from: "agent-2".into(),
        reason: Some("task done".into()),
        rejected: false,
    });
    insta::assert_snapshot!("shutdown_request_with_reason", s);
}

#[test]
fn shutdown_rejected() {
    let s = render_shutdown_to_string(ShutdownProps {
        from: "agent-2".into(),
        reason: Some("still working".into()),
        rejected: true,
    });
    insta::assert_snapshot!("shutdown_rejected", s);
}

#[test]
fn advisor_result_collapsed() {
    let s = render_advisor_to_string(AdvisorProps {
        kind: AdvisorKind::Result {
            text: "Looks good.".into(),
        },
        verbose: false,
    });
    insta::assert_snapshot!("advisor_result_collapsed", s);
}

#[test]
fn advisor_unavailable() {
    let s = render_advisor_to_string(AdvisorProps {
        kind: AdvisorKind::Error {
            error_code: "503".into(),
        },
        verbose: false,
    });
    insta::assert_snapshot!("advisor_unavailable", s);
}
