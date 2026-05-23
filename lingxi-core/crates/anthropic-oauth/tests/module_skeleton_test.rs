//! Smoke test: the new M3-04 modules compile and export the expected symbols.
//! This is the very first failing test of M3-04.

#[test]
fn refresh_module_exports_auth_state_and_driver() {
    fn _accepts_state(_: &lingxi_anthropic_oauth::refresh::AuthState) {}
    fn _accepts_driver(_: &lingxi_anthropic_oauth::refresh::RefreshDriver) {}
}

#[test]
fn scope_upgrade_module_exports_parser() {
    let _: Option<lingxi_anthropic_oauth::scope_upgrade::ScopeUpgradeRequired> =
        lingxi_anthropic_oauth::scope_upgrade::parse_scope_upgrade("{}");
}

#[test]
fn config_default_uses_spec_locked_endpoints() {
    use lingxi_anthropic_oauth::ClaudeAiOAuthConfig;
    let c = ClaudeAiOAuthConfig::default_with_port(0);
    assert_eq!(c.authorization_endpoint, "https://claude.ai/oauth/authorize");
    assert_eq!(c.token_endpoint, "https://console.anthropic.com/v1/oauth/token");
    // Scope order is locked.
    assert_eq!(
        c.scopes,
        vec![
            "read:user".to_string(),
            "write:messages".to_string(),
            "read:projects".to_string(),
        ],
    );
}
