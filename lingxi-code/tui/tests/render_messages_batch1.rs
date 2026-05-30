//! M7-04 batch-1 renderer snapshots.
use tui::components::messages::advisor::{render_advisor_to_string, AdvisorProps};
use tui::components::messages::compact_boundary::render_compact_boundary_to_string;
use tui::components::messages::hook_progress::{render_hook_progress_to_string, HookProgressProps};
use tui::components::messages::plan_approval::{render_plan_approval_to_string, PlanApprovalProps};
use tui::components::messages::rate_limit::{render_rate_limit_to_string, RateLimitProps};
use tui::components::messages::redacted_thinking::render_redacted_thinking_to_string;
use tui::components::messages::shutdown::{render_shutdown_to_string, ShutdownProps};
use tui::components::messages::system_api_error::{
    render_system_api_error_to_string, SystemApiErrorProps,
};
use tui::components::messages::system_text::{render_system_text_to_string, SystemTextProps};
use tui::components::messages::thinking::{render_thinking_to_string, ThinkingProps};
use tui::state::{AdvisorKind, PlanApprovalKind, SystemLevel};

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
        ..Default::default()
    });
    insta::assert_snapshot!("system_text_info_plain", s);
}

#[test]
fn system_text_warning_dotted() {
    let s = render_system_text_to_string(SystemTextProps {
        body: "Approaching context limit.".into(),
        level: SystemLevel::Warning,
        ..Default::default()
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
        ..Default::default()
    });
    insta::assert_snapshot!("shutdown_request_with_reason", s);
}

#[test]
fn shutdown_rejected() {
    let s = render_shutdown_to_string(ShutdownProps {
        from: "agent-2".into(),
        reason: Some("still working".into()),
        rejected: true,
        ..Default::default()
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
        ..Default::default()
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
        ..Default::default()
    });
    insta::assert_snapshot!("advisor_unavailable", s);
}

#[test]
fn hook_progress_running_plural() {
    let s = render_hook_progress_to_string(HookProgressProps {
        event: "SessionStart".into(),
        count: 3,
        transcript_summary: false,
        ..Default::default()
    });
    insta::assert_snapshot!("hook_progress_running_plural", s);
}

#[test]
fn hook_progress_transcript_singular() {
    let s = render_hook_progress_to_string(HookProgressProps {
        event: "PreToolUse".into(),
        count: 1,
        transcript_summary: true,
        ..Default::default()
    });
    insta::assert_snapshot!("hook_progress_transcript_singular", s);
}

#[test]
fn plan_approval_request() {
    let s = render_plan_approval_to_string(PlanApprovalProps {
        kind: PlanApprovalKind::Request {
            from: "agent-3".into(),
            plan_content: "1. Do X\n2. Do Y".into(),
            plan_file_path: Some("/tmp/plan.md".into()),
        },
        ..Default::default()
    });
    insta::assert_snapshot!("plan_approval_request", s);
}

#[test]
fn plan_approval_approved() {
    let s = render_plan_approval_to_string(PlanApprovalProps {
        kind: PlanApprovalKind::Approved { name: "you".into() },
        ..Default::default()
    });
    insta::assert_snapshot!("plan_approval_approved", s);
}

#[test]
fn plan_approval_rejected() {
    let s = render_plan_approval_to_string(PlanApprovalProps {
        kind: PlanApprovalKind::Rejected {
            name: "you".into(),
            feedback: Some("too risky".into()),
        },
        ..Default::default()
    });
    insta::assert_snapshot!("plan_approval_rejected", s);
}
