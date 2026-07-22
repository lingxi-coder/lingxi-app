//! Live version-facing identifier pins for the Claude Code 2.1.216 oracle.

/// The current parity target is exposed from one source of truth.
#[test]
fn version_const_is_2_1_216() {
    assert_eq!(traits::CLAUDE_CODE_VERSION, "2.1.216");
}

/// Child processes receive the 2.1.216 `AI_AGENT` identifier.
#[test]
fn ai_agent_env_value_is_2_1_216() {
    let derived = format!(
        "claude-code_{}_agent",
        traits::CLAUDE_CODE_VERSION.replace('.', "-")
    );
    assert_eq!(derived, "claude-code_2-1-216_agent");
}

/// WebFetch presents the 2.1.216 Claude-compatible user agent.
#[test]
fn web_fetch_user_agent_is_2_1_216() {
    let derived = format!(
        "Claude-User (claude-code/{}; +https://support.anthropic.com/)",
        traits::CLAUDE_CODE_VERSION
    );
    assert_eq!(
        derived,
        "Claude-User (claude-code/2.1.216; +https://support.anthropic.com/)"
    );
}
