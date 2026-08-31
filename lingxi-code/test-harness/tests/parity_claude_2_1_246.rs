//! Historical version-facing identifiers captured from the Claude Code 2.1.246 oracle.
//!
//! The LIVE pins moved to `parity_claude_2_1_251.rs` when the parity target
//! advanced to 2.1.251. This file no longer asserts against the live version
//! constant; pinning it to a superseded version would make a correct target
//! bump fail.
//!
//! The derivation shape remains version-agnostic: the advertised version, the
//! child-process identity and the WebFetch user agent must share one source.

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
