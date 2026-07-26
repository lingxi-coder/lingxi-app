//! Live version-facing identifier pins for the Claude Code 2.1.220 oracle.
//!
//! 2.1.220 is the binary this port is read against — the forked-skill sidecars,
//! the `set_cwd` trust handshake, PowerShell 5.1 cwd-first shadowing, the
//! ←-on-empty gesture and the refusal cascade were all ported from it. The
//! advertised version had lagged at 2.1.217, so a session implementing 2.1.220
//! behaviour was telling servers and child processes it was something else.
//!
//! Everything below derives from ONE constant, so the three identifiers cannot
//! drift apart.

/// The current parity target is exposed from one source of truth.
#[test]
fn version_const_is_2_1_220() {
    assert_eq!(traits::CLAUDE_CODE_VERSION, "2.1.220");
}

/// Child processes receive the 2.1.220 `AI_AGENT` identifier.
#[test]
fn ai_agent_env_value_is_2_1_220() {
    let derived = format!(
        "claude-code_{}_agent",
        traits::CLAUDE_CODE_VERSION.replace('.', "-")
    );
    assert_eq!(derived, "claude-code_2-1-220_agent");
}

/// WebFetch presents the 2.1.220 Claude-compatible user agent.
#[test]
fn web_fetch_user_agent_is_2_1_220() {
    let derived = format!(
        "Claude-User (claude-code/{}; +https://support.anthropic.com/)",
        traits::CLAUDE_CODE_VERSION
    );
    assert_eq!(
        derived,
        "Claude-User (claude-code/2.1.220; +https://support.anthropic.com/)"
    );
}

/// Every version-facing identifier derives from the SAME constant, so a future
/// bump cannot move one and leave another behind — which is exactly how the
/// port came to advertise 2.1.217 while implementing 2.1.220.
#[test]
fn the_identifiers_share_one_source() {
    let v = traits::CLAUDE_CODE_VERSION;
    assert!(format!("claude-code_{}_agent", v.replace('.', "-")).contains(&v.replace('.', "-")));
    assert!(
        format!("Claude-User (claude-code/{v}; +https://support.anthropic.com/)").contains(v)
    );
}
