//! Parity fixture: `SettingsJson` → `SandboxRuntimeConfig` conversion.
//!
//! Locks the shape of `convert_settings_to_runtime_config` against
//! claude-code's reference (`src/utils/sandbox/sandbox-adapter.ts:38-103`).
//! `Edit(...)`, `Read(...)`, `WebFetch(domain:...)` rules from
//! `permissions.allow/deny` are folded into
//! `filesystem.{allowWrite,denyWrite,allowRead,denyRead}` and
//! `network.allowedDomains` in the resulting `SandboxRuntimeConfig`.
//! `Bash(...)` rules are intentionally ignored here — their decision lives
//! in `should_use_sandbox` (see `crates/sandbox/src/decision.rs`).

use sandbox::policy_convert::{convert_settings_to_runtime_config, SandboxConvertContext};
use sandbox::runtime_config::SettingsJson;
use serde::Deserialize;
use serde_json::Value;
use test_harness::parity::load_fixture;

#[derive(Deserialize)]
struct Fixture {
    input_settings: Value,
    expected_runtime_config: Value,
}

#[test]
fn sandbox_config_conversion_matches_claude_code() {
    let fx: Fixture = load_fixture("sandbox_config_conversion");

    let settings: SettingsJson =
        serde_json::from_value(fx.input_settings).expect("input settings parse");
    let runtime =
        convert_settings_to_runtime_config(&settings, &SandboxConvertContext::default());
    let got = serde_json::to_value(&runtime).expect("runtime serializes");
    let want = fx.expected_runtime_config;

    // Compare on the fields the fixture exercises. Equality through
    // `serde_json::Value` avoids coupling to internal field order.
    for key in ["enabled", "failIfUnavailable"] {
        assert_eq!(
            got.get(key),
            want.get(key),
            "field {key} must match claude-code conversion exactly: got={:?} want={:?}",
            got.get(key),
            want.get(key)
        );
    }

    // Filesystem allow/deny lists: assert each path the fixture mentions is
    // present (extracted from `Edit(...)` / `Read(...)` rules). Path
    // ordering must match — claude-code emits rules in the order they
    // appear in `permissions.allow` / `permissions.deny`.
    for key in ["allowWrite", "denyWrite", "allowRead", "denyRead"] {
        let got_arr = got["filesystem"][key]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let want_arr = want["filesystem"][key]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            got_arr, want_arr,
            "filesystem.{key} must match claude-code conversion: got={got_arr:?} want={want_arr:?}",
        );
    }

    // Network allowed domains: ordered by `WebFetch(domain:...)` allow-rule
    // appearance. (Denied domains are not surfaced in `allowedDomains`.)
    let got_domains = got["network"]["allowedDomains"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let want_domains = want["network"]["allowedDomains"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        got_domains, want_domains,
        "network.allowedDomains must match claude-code conversion: got={got_domains:?} want={want_domains:?}",
    );
}
