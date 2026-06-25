//! M7-04 Task 11: every batch-1 variant routes through `render_entry_to_string`
//! to its renderer's output. Guards the dispatch table.
use tui::components::messages::render_entry_to_string;
use tui::state::{AdvisorKind, PlanApprovalKind, RenderedMessage, SystemLevel};

#[test]
fn each_variant_routes_to_its_renderer() {
    let cases: Vec<(RenderedMessage, &str)> = vec![
        (
            RenderedMessage::AssistantThinking {
                thinking: "x".into(),
                expanded: false,
            },
            "\u{2234} Thinking (ctrl+o to expand)",
        ),
        (
            RenderedMessage::AssistantRedactedThinking,
            "\u{273B} Thinking\u{2026}",
        ),
        (
            RenderedMessage::CompactBoundary {
                messages_before: 50,
                messages_after: 5,
            },
            "\n\u{273B} Conversation compacted (ctrl+o for history)\n",
        ),
        (
            RenderedMessage::SystemTextRich {
                body: "hi".into(),
                level: SystemLevel::Info,
            },
            "hi",
        ),
        (
            RenderedMessage::SystemApiError {
                error: "529 Overloaded".into(),
                retry_attempt: 4,
                retry_in_seconds: 3,
                max_retries: 10,
                truncated: false,
            },
            "529 Overloaded\nRetrying in 3 seconds\u{2026} (attempt 4/10)",
        ),
        (
            RenderedMessage::RateLimit {
                text: "limited".into(),
                upsell: None,
            },
            "  \u{23BF}  limited",
        ),
        (
            RenderedMessage::Shutdown {
                from: "agent-2".into(),
                reason: Some("task done".into()),
                rejected: false,
            },
            "Shutdown request from agent-2\nReason: task done",
        ),
        (
            RenderedMessage::Advisor {
                kind: AdvisorKind::Error {
                    error_code: "503".into(),
                },
                verbose: false,
            },
            "Advisor unavailable (503)",
        ),
        (
            RenderedMessage::HookProgress {
                event: "PreToolUse".into(),
                count: 1,
                transcript_summary: true,
            },
            "  \u{23BF}  1 PreToolUse hook ran",
        ),
        (
            RenderedMessage::PlanApproval {
                kind: PlanApprovalKind::Approved { name: "you".into() },
            },
            "\u{2713} Plan Approved by you\nYou can now proceed with implementation. \
             Your plan mode restrictions have been lifted.",
        ),
    ];
    for (msg, expected) in cases {
        assert_eq!(
            render_entry_to_string(&msg, false, false),
            expected,
            "variant: {msg:?}"
        );
    }
}
