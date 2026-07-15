//! Parity regression pins vs Claude Code 2.1.208.
//!
//! This file owns the current version-facing identifiers. Older version-named
//! harnesses remain as historical behavior regressions, but must not pin the
//! live `traits::CLAUDE_CODE_VERSION`.

/// The current parity target single source of truth.
#[test]
fn version_const_is_2_1_208() {
    assert_eq!(
        traits::CLAUDE_CODE_VERSION,
        "2.1.208",
        "parity-target version const must track the 2.1.208 wave"
    );
}

/// The child-process `AI_AGENT` env value:
/// `claude-code_${VERSION.replace(/\./g,"-")}_agent`.
#[test]
fn ai_agent_env_value_is_2_1_208() {
    let derived = format!(
        "claude-code_{}_agent",
        traits::CLAUDE_CODE_VERSION.replace('.', "-")
    );
    assert_eq!(derived, "claude-code_2-1-208_agent");
    let mid = derived
        .strip_prefix("claude-code_")
        .and_then(|s| s.strip_suffix("_agent"))
        .expect("prefix/suffix present");
    assert!(!mid.contains('.'), "version dots must be dashed: {derived}");
}

/// The WebFetch `User-Agent`:
/// `Claude-User (claude-code/${VERSION}; +https://support.anthropic.com/)`.
#[test]
fn web_fetch_user_agent_is_2_1_208() {
    let derived = format!(
        "Claude-User (claude-code/{}; +https://support.anthropic.com/)",
        traits::CLAUDE_CODE_VERSION
    );
    assert_eq!(
        derived,
        "Claude-User (claude-code/2.1.208; +https://support.anthropic.com/)"
    );
}
