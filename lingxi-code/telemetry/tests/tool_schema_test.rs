use telemetry::tengu::tool;

#[test]
fn all_40_tool_event_names_are_locked() {
    let names: &[&str] = &[
        tool::STARTED,
        tool::COMPLETED,
        tool::FAILED,
        tool::CANCELLED,
        tool::PERMISSION_REQUESTED,
        tool::PERMISSION_GRANTED,
        tool::PERMISSION_DENIED,
        tool::PERMISSION_REMEMBERED,
        tool::BASH_STARTED,
        tool::BASH_COMPLETED,
        tool::BASH_FAILED,
        tool::BASH_TIMEOUT,
        tool::EDIT_STARTED,
        tool::EDIT_COMPLETED,
        tool::EDIT_FAILED,
        tool::READ_STARTED,
        tool::READ_COMPLETED,
        tool::READ_FAILED,
        tool::WRITE_STARTED,
        tool::WRITE_COMPLETED,
        tool::WRITE_FAILED,
        tool::GREP_STARTED,
        tool::GREP_COMPLETED,
        tool::GREP_FAILED,
        tool::GLOB_STARTED,
        tool::GLOB_COMPLETED,
        tool::GLOB_FAILED,
        tool::WEB_FETCH_STARTED,
        tool::WEB_FETCH_COMPLETED,
        tool::WEB_FETCH_FAILED,
        tool::TASK_DISPATCHED,
        tool::TASK_COMPLETED,
        tool::TASK_FAILED,
        tool::NOTEBOOK_STARTED,
        tool::NOTEBOOK_COMPLETED,
        tool::NOTEBOOK_FAILED,
        tool::MCP_INVOKED,
        tool::MCP_COMPLETED,
        tool::MCP_FAILED,
        tool::SKILL_INVOKED,
    ];
    assert_eq!(
        names.len(),
        40,
        "tool category must declare exactly 40 events"
    );
    for n in names {
        assert!(
            n.starts_with("tengu_tool_"),
            "{n} must start with tengu_tool_"
        );
    }
}

#[test]
fn bash_started_payload_uses_pii_tagged_for_command() {
    use telemetry::pii::PiiTagged;
    let p = tool::BashStartedPayload {
        invocation_id: telemetry::Verified::assert_safe("inv-1".into()),
        command: PiiTagged::assert_pii_tagged_column("echo hi".into()),
        timeout_ms: 30_000,
    };
    let json = serde_json::to_string(&p).expect("serialize");
    let _: tool::BashStartedPayload = serde_json::from_str(&json).expect("round-trip");
}
