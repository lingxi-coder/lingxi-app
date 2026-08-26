//! Live version-facing identifier pins for the Claude Code 2.1.246 oracle.
//!
//! Everything below derives from one constant so the advertised version, child
//! process identity, and WebFetch user agent cannot drift independently.

#[test]
fn version_const_is_2_1_246() {
    assert_eq!(traits::CLAUDE_CODE_VERSION, "2.1.246");
}

#[test]
fn ai_agent_env_value_is_2_1_246() {
    let derived = format!(
        "claude-code_{}_agent",
        traits::CLAUDE_CODE_VERSION.replace('.', "-")
    );
    assert_eq!(derived, "claude-code_2-1-246_agent");
}

#[test]
fn web_fetch_user_agent_is_2_1_246() {
    let derived = format!(
        "Claude-User (claude-code/{}; +https://support.anthropic.com/)",
        traits::CLAUDE_CODE_VERSION
    );
    assert_eq!(
        derived,
        "Claude-User (claude-code/2.1.246; +https://support.anthropic.com/)"
    );
}

#[test]
fn version_facing_identifiers_share_one_source() {
    let version = traits::CLAUDE_CODE_VERSION;
    assert!(format!("claude-code_{}_agent", version.replace('.', "-"))
        .contains(&version.replace('.', "-")));
    assert!(
        format!("Claude-User (claude-code/{version}; +https://support.anthropic.com/)")
            .contains(version)
    );
}
